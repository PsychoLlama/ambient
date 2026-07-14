//! Cross-module registration (Phase 1, first half): foreign package items,
//! imports, and the import environment.

use std::collections::HashSet;
use std::sync::Arc;

use crate::ast::BindingId;
use crate::fqn::Fqn;
use crate::module_path::ModulePath;
use crate::module_registry::{ExportKind, ModuleRegistry, ResolvedImport};
use crate::types::Type;

use crate::infer::env::{Scheme, TypeEnv};
use crate::infer::error::{BoxedTypeError, TypeError, TypeErrorKind};
use crate::infer::{Infer, inherent};

use super::abilities::{
    register_imported_ability, seed_namespaced_ability_dynamics, seed_prelude_struct_aliases,
};
use super::impls::{build_inherent_method_scheme, inherent_impl_target};
use super::locals::{
    build_function_scheme, named_type_def, register_named_types, register_trait_def,
};

/// Register the cross-module context for a package build: platform dynamics,
/// foreign package items, and imports, returning the resulting import env.
///
/// Runs as the first half of Phase 1 when checking a module inside a
/// registry (as opposed to a standalone single-file check).
pub(super) fn register_cross_module(
    infer: &mut Infer,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    errors: &mut Vec<BoxedTypeError>,
) -> TypeEnv {
    // Seed the prelude's `extern` struct types (primitive nominals and the
    // `List`/`Map`/`Set` heads) *first*: the next step resolves every
    // module's ability signatures (which name primitives like
    // `String`/`Number` and containers like `List<String>`), and it runs
    // before `register_package_items` populates the general alias table.
    // Without this, such a name in an ability signature would resolve bare
    // and corrupt the ability's method signature hashes — the same reason
    // `resolve_ability_declarations` seeds them. Only the prelude's `extern`
    // structs are seeded here; every other type still resolves at its use
    // site once the full alias table is built below.
    seed_prelude_struct_aliases(infer, registry);
    // Every module's abilities are always in scope fully-qualified. Seed
    // them as namespaced dynamics before imports and local abilities
    // resolve, so `core::system::Stdio` / `pkg::effects::Counter`
    // references (inline uses and cross-module ability deps) and the
    // `use core::system::Tcp;` bridge all find their target — on every
    // path that has a registry, including the package build.
    seed_namespaced_ability_dynamics(infer, registry, errors);
    // Imported enums register first: foreign impl registration (next)
    // must resolve an imported enum target to its uuid, or the impl's
    // dispatch key won't match the call sites'.
    register_imported_enums(infer, module_path, registry);
    // Every *other* public enum registers into the qualified-only channel:
    // a fully-qualified variant reference (`pkg::m::Enum::Variant`, needing
    // no `use`) resolves through it by identity. Bare names stay untouched,
    // so this can't leak an un-imported enum into scope.
    register_qualified_foreign_enums(infer, module_path, registry);
    // Imported trait *definitions* register next: like enums, they are
    // import-scoped, so a module sees only the traits it can name (via `use`
    // or the prelude). Impl coherence stays build-global below.
    register_imported_traits(infer, module_path, registry);
    // Make the rest of the package's types, traits, and impls visible
    // (signatures only). Runs before local registration and import
    // resolution so imported signatures resolve foreign nominal types.
    register_package_items(infer, module_path, registry, errors);
    let env = build_import_env(infer, module_path, registry, errors);
    // `register_package_items` registered *every* foreign module's type
    // aliases so foreign impl targets and imported signatures could
    // resolve to the right nominal identity. Those schemes are now
    // hydrated into `env`, so retract the foreign aliases this module
    // didn't explicitly `use` — otherwise their bare names would resolve
    // in this module's own bodies regardless of `pub` or `use`. Traits
    // and impls stay build-global for coherence; nominal *types* follow
    // the same visibility rules as values.
    retain_imported_type_aliases(infer, module_path, registry);
    env
}
/// Register the enums a module imports (`use pkg::m::{SomeEnum}`) into the
/// enum registry, as if they were declared locally: the type name resolves,
/// and `register_enums` later binds their variant constructors and patterns
/// alongside the local ones. Local declarations register afterwards, so
/// they shadow imported variants — the same precedence the compiler applies.
///
/// The walk itself is [`crate::module_env::imported_enum_defs`], shared with
/// [`crate::module_env::ModuleEnv::new`]: the compiler's imported-enum
/// channel is the same collection by construction, so the checker and the
/// compiler cannot disagree about which enums a module imports.
fn register_imported_enums(
    infer: &mut Infer,
    current_module: &ModulePath,
    registry: &ModuleRegistry,
) {
    for (enum_module, def) in crate::module_env::imported_enum_defs(registry, current_module) {
        infer.enum_registry.register_def(&def, Some(enum_module));
    }
}
/// Register every *other* module's public enums into the enum registry's
/// qualified-only channel ([`crate::infer::enums::EnumRegistry::get_qualified`]).
///
/// The pattern checker resolves a variant against the enum named by its
/// resolved `Fqn`'s declaring module — but only imported enums register by
/// bare name, so a fully-qualified reference to an *un-imported* foreign enum
/// (`pkg::m::Enum::Variant`, `m::Variant`) would otherwise miss and fall to
/// the bare-name reverse lookup, which a same-named local variant could
/// hijack. This channel closes that gap by identity, mirroring the compiler's
/// [`crate::module_env::ModuleEnv::foreign_enum_variants`]: every public enum
/// keyed by `(module, name)`, never bare, so no un-imported name leaks into
/// scope. Idempotent with [`register_imported_enums`] (same `(module, name)`
/// key, same definition).
fn register_qualified_foreign_enums(
    infer: &mut Infer,
    current_module: &ModulePath,
    registry: &ModuleRegistry,
) {
    for info in registry.all_modules() {
        if &info.path == current_module {
            continue;
        }
        let enum_module = registry.module_id(&info.path);
        for item in &info.module.items {
            if let crate::ast::ItemKind::Enum(enum_def) = &item.kind
                && enum_def.is_public
            {
                let enum_info =
                    crate::infer::enums::EnumInfo::from_def(enum_def, Some(enum_module.clone()));
                infer.enum_registry.register_qualified(&Arc::new(enum_info));
            }
        }
    }
}
/// Register the traits a module imports (`use pkg::m::{SomeTrait}`, or the
/// prelude operator traits) into the trait registry, as if declared locally.
///
/// Trait *definitions* are import-scoped: only the traits a module can name
/// (via `use` or the prelude) register here, so `Default` — omitted from the
/// prelude — is unavailable without `use core::traits::Default`. Impl
/// coherence stays build-global (`register_package_items`), keying off trait
/// *name*, so an imported trait's impls are still visible for dispatch.
///
/// The walk is [`crate::module_env::imported_trait_defs`] — the shared
/// import-collection path, like [`register_imported_enums`].
fn register_imported_traits(
    infer: &mut Infer,
    current_module: &ModulePath,
    registry: &ModuleRegistry,
) {
    for (trait_module, def) in crate::module_env::imported_trait_defs(registry, current_module) {
        register_trait_def(infer, &def, Some(&trait_module));
    }
}
/// Register the types, traits, and impls declared in the *other* modules of
/// the package so they can be resolved while checking this module.
///
/// Foreign items are registered by signature only — their bodies were (or
/// will be) checked in their own module's check pass. Impls register the
/// dispatch mapping `(trait, type uuid) → method symbol`; the symbols are
/// resolved to content hashes at link time like any function name.
///
/// Trait *impls* stay build-global for coherence (loop 2 below keys dispatch
/// off trait *name*), but trait *definitions* are import-scoped — they
/// register via [`register_imported_traits`], not here. The foreign *type
/// aliases* registered here are transient: they exist so foreign impl
/// targets and imported signatures resolve to the right nominal identity.
/// [`retain_imported_type_aliases`] retracts the ones this module didn't
/// import once that resolution is done, so they can't be named by bare
/// identifier. This runs before the current module's own registrations, so
/// local declarations shadow foreign ones on name collisions.
fn register_package_items(
    infer: &mut Infer,
    current_module: &ModulePath,
    registry: &ModuleRegistry,
    errors: &mut Vec<BoxedTypeError>,
) {
    let foreign_modules: Vec<_> = registry
        .all_modules()
        .filter(|info| &info.path != current_module)
        .collect();

    // Every package trait — *including the current module's own* — indexes
    // its `Fqn` in the build-global table first. Imported trait defs already
    // registered via `register_imported_traits`; every *other* foreign trait
    // registers by identity only (no bare name in scope), so foreign impls
    // and the bounds of foreign signatures resolve even when this module
    // never imported the trait.
    //
    // The current module is included on purpose: `build_import_env` (next)
    // builds the *foreign* public functions' schemes, and a foreign `pub fn
    // f<T: Local>(…)` bounded on a trait declared *here* must resolve its
    // bound before this module's own `register_local_declarations` runs. The
    // current module's traits re-register named there (idempotent by uuid);
    // their bare-name scope entry and declaration validation still come from
    // that later pass.
    for info in registry.all_modules() {
        let trait_module = registry.module_id(&info.path);
        for item in &info.module.items {
            if let crate::ast::ItemKind::Trait(trait_def) = &item.kind {
                super::locals::register_trait_def_unnamed(infer, trait_def, Some(&trait_module));
            }
        }
    }
    for info in &foreign_modules {
        // Foreign types register bare (transient, retracted later); their
        // *public* `Item(Fqn)` keys come from the loop just below, so the
        // bare-only registration here passes `None`.
        register_named_types(infer, &info.module, None);
        // Public named types also register under their canonical qualified key
        // (`shapes.Money`): the resolve pass rewrites qualified type
        // constructors (`pkg::shapes::Money { … }`) to that key, and canonical
        // keys are never retracted (they can't leak as bare names).
        for item in &info.module.items {
            if let Some((name, target, true)) = named_type_def(item) {
                let fqn = registry.fqn(&info.path, &[Arc::clone(name)]);
                infer.register_type_alias_item(fqn, target);
            }
        }
    }

    for info in &foreign_modules {
        for item in &info.module.items {
            if let crate::ast::ItemKind::Impl(impl_def) = &item.kind {
                match &impl_def.trait_name {
                    Some(trait_name) => {
                        register_foreign_impl(infer, impl_def, trait_name, errors);
                    }
                    None => register_foreign_inherent_impl(infer, impl_def, errors),
                }
            }
        }
    }
}
/// Register the dispatch mapping for a trait impl defined in another module.
///
/// Skips silently on unresolvable traits or non-nominal types: the impl's
/// own module reports those errors during its check pass.
fn register_foreign_impl(
    infer: &mut Infer,
    impl_def: &crate::ast::ImplDef,
    trait_name: &crate::ast::QualifiedName,
    errors: &mut Vec<BoxedTypeError>,
) {
    let Some(trait_uuid) = infer.trait_uuid_of(trait_name) else {
        return;
    };

    // Mirror `check_single_impl` exactly so the dispatch key (and now the
    // conditional-impl target/bounds) a foreign impl registers matches what
    // call sites resolve. A conditional (generic) impl resolves its target
    // under its own rigid type parameters, so `Pair<T>` retains `Param(T)`.
    let is_generic = !impl_def.type_params.is_empty()
        || matches!(&impl_def.for_type, Type::Named(n) if !n.args.is_empty());
    let rigid: Vec<Arc<str>> = impl_def
        .type_params
        .iter()
        .filter(|tp| !tp.is_ability)
        .map(|tp| Arc::clone(&tp.name))
        .collect();
    let for_type = infer.with_rigid_params(rigid, |infer| infer.resolve_holes(&impl_def.for_type));

    // Non-identity targets are skipped silently: the defining module reports
    // them during its own check pass.
    let Some((type_uuid, type_name)) = inherent::trait_impl_identity(&for_type) else {
        return;
    };

    let mut impl_record = crate::types::TraitImpl::new(trait_uuid, type_uuid, type_name);
    if is_generic {
        // Bounds resolve by the `Fqn` the resolve pass wrote in the impl's
        // own module; unknown-trait errors are the defining module's to
        // report, so swallow them here.
        let bounds = infer.resolve_bound_params(&impl_def.type_params, &mut Vec::new());
        impl_record = impl_record.with_generic_target(for_type.clone(), bounds);
    }
    for method in &impl_def.methods {
        let symbol = crate::types::impl_method_symbol(&type_uuid, &trait_uuid, &method.name);
        impl_record.methods.insert(Arc::clone(&method.name), symbol);
    }
    if infer.trait_registry.register_impl(impl_record).is_some() {
        // Two other modules implement the same trait for the same type.
        // Their dispatch symbols collide, so this is unresolvable ambiguity.
        errors.push(Box::new(TypeError::new(
            TypeErrorKind::DuplicateImpl {
                trait_name: Arc::clone(&trait_name.name),
                ty: for_type.clone(),
            },
            (impl_def.span.start, impl_def.span.end),
        )));
    }
}
/// Register the dispatch mapping for an inherent impl defined in another
/// module.
///
/// Skips silently on invalid targets (the impl's own module reports those),
/// and performs no enum-name validation — foreign enums aren't registered
/// while checking this module. Duplicate method registrations are reported:
/// two modules defining the same method for the same type is unresolvable
/// ambiguity, exactly like a duplicate trait impl.
fn register_foreign_inherent_impl(
    infer: &mut Infer,
    impl_def: &crate::ast::ImplDef,
    errors: &mut Vec<BoxedTypeError>,
) {
    let Some((key, for_type)) = inherent_impl_target(infer, impl_def) else {
        return;
    };
    let impl_type_params = impl_def.type_params.clone();
    for method in &impl_def.methods {
        // Signature problems (e.g. a missing return type) are the defining
        // module's errors; swallow them here.
        let mut scratch = Vec::new();
        let scheme =
            build_inherent_method_scheme(infer, &impl_type_params, method, &for_type, &mut scratch);
        let symbol = inherent::inherent_method_symbol(&key, &method.name);
        let record = inherent::InherentMethod {
            name: Arc::clone(&method.name),
            has_self: method.has_self,
            scheme,
            symbol,
        };
        if infer
            .inherent_registry
            .register(key.clone(), record)
            .is_some()
        {
            errors.push(Box::new(TypeError::new(
                TypeErrorKind::DuplicateInherentMethod {
                    method: Arc::clone(&method.name),
                    ty: for_type.clone(),
                },
                (impl_def.span.start, impl_def.span.end),
            )));
        }
    }
}
/// Retract the foreign type aliases that leaked into the alias table during
/// package registration, keeping only the ones this module imports by name.
///
/// [`register_package_items`] registers every foreign module's type aliases
/// so foreign impl targets and imported function/const signatures resolve to
/// the right nominal identity. That same table backs bare-name type
/// resolution in this module's own bodies (see [`Infer::resolve_holes`]), so
/// leaving the foreign entries in would let code name any package type
/// without a `use` and regardless of its visibility — undermining `pub`.
///
/// This must run after [`build_import_env`] (which needs the full foreign set
/// to hydrate imported schemes) and before local aliases register, so the
/// table holds only foreign entries: retaining the imported names drops
/// exactly the leaked ones. Private foreign aliases can't be imported
/// (`resolve_imports` rejects them), so they're dropped here too.
fn retain_imported_type_aliases(
    infer: &mut Infer,
    current_module: &ModulePath,
    registry: &ModuleRegistry,
) {
    // A failed resolution already surfaced diagnostics in `build_import_env`;
    // with no trustworthy import list, drop every foreign alias rather than
    // keep leaking them.
    let imported: HashSet<Arc<str>> = registry
        .resolve_imports(current_module)
        .map(|resolved| {
            resolved
                .imports
                .into_iter()
                .filter_map(|(name, bindings)| {
                    bindings
                        .iter()
                        .any(|import| {
                            matches!(
                                import,
                                ResolvedImport::Symbol {
                                    export_kind: ExportKind::Struct | ExportKind::TypeAlias,
                                    ..
                                }
                            )
                        })
                        .then_some(name)
                })
                .collect()
        })
        .unwrap_or_default();
    // Cross-module `Item` keys always stay: they can't collide with bare
    // names, and qualified references are visibility-checked by the resolve
    // pass. Among bare keys, keep only the imported ones. The four
    // primitives (`Bool`/`Number`/`String`/`Binary`) are no longer special-
    // cased here: they arrive as ordinary prelude imports (see
    // `core::prelude`), so they land in `imported` on every module that
    // doesn't shadow them, exactly like any other imported type.
    infer.retain_type_aliases(|key| match key {
        crate::fqn::NameKey::Item(_) => true,
        crate::fqn::NameKey::Bare(name) => imported.contains(name),
    });
}
/// Build a type environment from imported modules.
///
/// This processes the imports in the module and adds type schemes for
/// each imported symbol to the environment.
fn build_import_env(
    infer: &mut Infer,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    errors: &mut Vec<BoxedTypeError>,
) -> TypeEnv {
    let mut env = TypeEnv::new();
    let mut next_binding_id: BindingId = 2_000_000; // Start high to avoid collisions

    // Get resolved imports for this module
    let imports = match registry.resolve_imports(module_path) {
        Ok(resolved) => {
            // Each failed import is a real diagnostic at its `use` item:
            // a missing module, a missing symbol, or a private symbol.
            for failed in resolved.errors {
                errors.push(Box::new(TypeError::new(
                    TypeErrorKind::ImportFailed {
                        message: failed.error.to_string(),
                    },
                    (failed.span.start, failed.span.end),
                )));
            }
            resolved.imports
        }
        Err(e) => {
            // Module not in registry - return empty env
            // This can happen for the root module being checked
            errors.push(Box::new(TypeError::new(
                TypeErrorKind::CannotInfer {
                    hint: format!("import resolution failed: {e}"),
                },
                (0, 0),
            )));
            return env;
        }
    };

    for (name, bindings) in imports {
        // Value imports need no bare env binding: the resolve pass rewrote
        // every reference to an imported function or const to its canonical
        // qualified key, which `bind_all_module_exports` covers. The
        // channels that survive here carry information a scheme can't:
        // enum declarations (constructors and patterns) and abilities
        // (interface identities).
        for resolved_import in bindings {
            match resolved_import {
                ResolvedImport::Module(_)
                | ResolvedImport::Symbol {
                    export_kind:
                        ExportKind::Enum
                        | ExportKind::Function
                        | ExportKind::Const
                        | ExportKind::Struct
                        | ExportKind::TypeAlias
                        | ExportKind::Trait,
                    ..
                } => {
                    // Modules resolve through paths; enums are registered by
                    // `register_imported_enums`; values resolve canonically;
                    // types/traits register through `register_package_items`.
                }
                ResolvedImport::Symbol {
                    export_kind: ExportKind::EnumVariant,
                    span,
                    ..
                } => {
                    // Variants don't import piecemeal: pattern matching and
                    // constructor tags need the whole declaration.
                    errors.push(Box::new(TypeError::new(
                        TypeErrorKind::ImportFailed {
                            message: format!(
                                "`{name}` is an enum variant; import its enum instead"
                            ),
                        },
                        (span.start, span.end),
                    )));
                }
                ResolvedImport::Symbol {
                    export_kind: ExportKind::Ability,
                    from_module,
                    ..
                } => register_imported_ability(infer, registry, &from_module, &name, errors),
                ResolvedImport::Symbol {
                    export_kind: ExportKind::AbilityMethod,
                    ..
                } => {
                    // An imported ability method binds nothing in the type
                    // env: the resolve pass canonicalizes each bare
                    // `method!(…)` perform to its ability's `Fqn`, and the
                    // namespaced dynamics (seeded for every registered
                    // module) carry the interface.
                }
            }
        }
    }

    bind_all_module_exports(infer, module_path, registry, &mut env, &mut next_binding_id);
    bind_foreign_enum_variants(infer, module_path, registry, &mut env, &mut next_binding_id);

    env
}
/// Bind every registered module's public exports into `env` under their
/// canonical qualified names (`core.List.map`, `utils.helper`,
/// `deep.nested.leaf.leaf_fn`).
///
/// This is the single environment-side convention the resolve pass
/// targets: a reference — however it was spelled — resolves to its
/// canonical key, and that key is bound here. The current module is
/// skipped: its own items bind bare (references into it normalize to the
/// bare form), and unannotated private functions don't hydrate as foreign
/// schemes.
fn bind_all_module_exports(
    infer: &mut Infer,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    env: &mut TypeEnv,
    next_binding_id: &mut BindingId,
) {
    for module_info in registry.all_modules() {
        let path = module_info.path.clone();
        if &path == module_path {
            continue;
        }
        for export in registry.get_public_exports(&path) {
            if let Some(scheme) =
                get_symbol_scheme(infer, &module_info.module, &export.name, export.kind)
            {
                let fqn = registry.fqn(&path, &[Arc::clone(&export.name)]);
                let binding_id = *next_binding_id;
                *next_binding_id += 1;
                env.insert_item(binding_id, fqn, scheme);
            }
        }
    }
}
/// Bind every *public* foreign enum's variant constructors under their
/// canonical two-segment `Fqn(enum_module, [Enum, Variant])` — the key a
/// fully-qualified (`core::option::Some`) or explicit-enum
/// (`pkg::shapes::Shape::Circle`) reference resolves to.
///
/// Fqn-only, never bare: same-module variants, enum-imported variants, and
/// the prelude come through `register_enums`; this fills the qualified
/// channel none of those cover. Mirrors [`bind_all_module_exports`] — every
/// public enum in every *other* module, keyed by `Fqn`.
fn bind_foreign_enum_variants(
    infer: &mut Infer,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    env: &mut TypeEnv,
    next_binding_id: &mut BindingId,
) {
    for module_info in registry.all_modules() {
        let path = module_info.path.clone();
        if &path == module_path {
            continue;
        }
        let module_id = registry.module_id(&path);
        for item in &module_info.module.items {
            let crate::ast::ItemKind::Enum(enum_def) = &item.kind else {
                continue;
            };
            if !enum_def.is_public {
                continue;
            }
            let info = crate::infer::enums::EnumInfo::from_def(enum_def, Some(module_id.clone()));
            for idx in 0..info.variants.len() {
                let scheme = info.constructor_scheme(&mut infer.r#gen, idx);
                let fqn = Fqn::new(
                    module_id.clone(),
                    vec![Arc::clone(&info.name), Arc::clone(&info.variants[idx].name)],
                );
                env.insert_item(*next_binding_id, fqn, scheme);
                *next_binding_id += 1;
            }
        }
    }
}
/// Get the type scheme for a symbol from a module's AST.
///
/// Only functions and consts hydrate as value schemes. Enums don't: item
/// imports register the whole definition into the enum registry (see
/// `register_imported_enums`), and whole-module imports skip them — a
/// qualified `alias::Variant` would type-check here but has no
/// compile-time constructor entry, so binding it would trade a type error
/// for a link failure.
fn get_symbol_scheme(
    infer: &mut Infer,
    module: &crate::ast::Module,
    name: &str,
    kind: ExportKind,
) -> Option<Scheme> {
    for item in &module.items {
        match (&item.kind, kind) {
            (crate::ast::ItemKind::Function(func), ExportKind::Function)
                if func.name.as_ref() == name =>
            {
                // Foreign function: no ability inference — an absent
                // `with` clause on an export means pure. Its bounds resolve
                // by the `Fqn` the resolve pass wrote in the *defining*
                // module, so no consumer-scope re-resolution is needed.
                return Some(build_function_scheme(infer, func, false));
            }
            // A foreign extern fn exports as a Function; its declared
            // signature is the whole contract (always pure).
            (crate::ast::ItemKind::ExternFn(def), ExportKind::Function)
                if def.name.as_ref() == name =>
            {
                return Some(super::locals::build_extern_fn_scheme(infer, def));
            }
            (crate::ast::ItemKind::Const(const_def), ExportKind::Const)
                if const_def.name.as_ref() == name =>
            {
                let ty = const_def
                    .ty
                    .clone()
                    .or_else(|| crate::const_eval::literal_type(&const_def.value))
                    .unwrap_or(Type::Unit);
                return Some(Scheme::mono(ty));
            }
            // A foreign unit struct is a value too: bind its bare-name
            // constructor under the canonical `<module>::Origin` key (the
            // caller keys off this) so imported/qualified value references
            // type-check. `s.ty` is the `Type::Nominal`, carrying identity.
            (crate::ast::ItemKind::Struct(s), ExportKind::Struct)
                if s.name.as_ref() == name && s.is_unit_value() =>
            {
                return Some(Scheme::mono(s.ty.clone()));
            }
            _ => {}
        }
    }
    None
}

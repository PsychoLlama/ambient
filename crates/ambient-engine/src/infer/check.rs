//! Module-level type checking.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::ability_resolver::AbilityResolver;
use crate::ast::BindingId;
use crate::module_path::ModulePath;
use crate::module_registry::{ExportKind, ModuleRegistry, ResolvedImport};
use crate::types::{AbilityId, AbilitySet, TraitDef, TraitMethodDef, Type, TypeVarId};

use super::Infer;
use super::env::{Scheme, TypeEnv};
use super::error::{BoxedTypeError, BoxedTypeErrorExt, TypeError, TypeErrorKind};
use super::expr::substitute_self;
use super::inherent;

/// Result of type checking a module.
#[derive(Debug)]
pub struct CheckResult {
    /// Type errors found during checking.
    pub errors: Vec<BoxedTypeError>,
    /// The typed module (with types filled in on expressions).
    pub module: crate::ast::Module,
}

impl CheckResult {
    /// Returns true if there were no errors.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Returns the errors, consuming the result.
    #[must_use]
    pub fn into_errors(self) -> Vec<BoxedTypeError> {
        self.errors
    }
}

/// Check a module for type errors.
///
/// This function performs module-level type inference:
/// 1. Collects all function signatures into the type environment
/// 2. Type-checks each function body
/// 3. Verifies return types match declared types
/// 4. Returns all accumulated type errors
///
/// # Example
///
/// ```ignore
/// let module = ambient_parser::parse(source)?;
/// let result = check_module(module);
/// if !result.is_ok() {
///     for error in &result.errors {
///         eprintln!("Type error: {}", error);
///     }
/// }
/// ```
#[must_use]
pub fn check_module(module: crate::ast::Module) -> CheckResult {
    check_module_core(Infer::new(), module, None)
}

/// The shared checking pipeline behind all `check_module*` entry points.
///
/// Phases:
/// 1. Registration — foreign package items (if cross-module), imports, local
///    type aliases, traits, and function signatures.
/// 2. Impl blocks — method bodies checked, dispatch symbols assigned.
/// 3. Function/const bodies — types inferred; each unannotated private
///    function's ability variable is bound to its body's inferred effects.
/// 4. Ability enforcement — deferred until all bodies are checked so calls
///    to functions defined later (whose ability variables bind in phase 3)
///    resolve before their callers are judged.
fn check_module_core(
    mut infer: Infer,
    mut module: crate::ast::Module,
    cross_module: Option<(&ModulePath, &ModuleRegistry)>,
) -> CheckResult {
    let mut errors = Vec::new();

    // Phase 0: canonicalize cross-module references. Every import, module
    // alias, and inline rooted path is resolved to its one fully-qualified
    // identity (`QualifiedName::resolved`); everything downstream keys off
    // that. Module-level import failures are reported from the module
    // scope in `build_import_env`; block-scoped `use` failures only exist
    // here.
    if let Some((module_path, registry)) = cross_module {
        let outcome = crate::resolve::resolve_module(&mut module, module_path, registry);
        for failed in outcome.errors {
            errors.push(Box::new(TypeError::new(
                TypeErrorKind::ImportFailed {
                    message: failed.error.to_string(),
                },
                (failed.span.start, failed.span.end),
            )));
        }
    }

    // Phase 1: registration.
    let mut env = match cross_module {
        Some((module_path, registry)) => {
            register_cross_module(&mut infer, module_path, registry, &mut errors)
        }
        None => TypeEnv::new(),
    };

    register_named_types(&mut infer, &module);
    // Unit structs are values as well as types: each denotes a single value
    // constructed by its bare name (like a nullary enum variant). Bind that
    // value into the env so `let o = Origin` type-checks.
    register_unit_struct_values(&module, &mut env);
    register_traits(&mut infer, &module);
    register_enums(&mut infer, &module, &mut env, &mut errors);
    // ORDERING (load-bearing): `build_import_env` (above) already registered
    // cross-module ability imports as bare dynamics; `register_abilities`
    // runs *after* and overwrites, so a local `ability` shadows an imported
    // one of the same bare name (matching the value/type shadowing rule).
    register_abilities(&mut infer, &mut module, &mut errors);
    collect_function_signatures(&mut infer, &module, &mut env);
    // Constants register alongside functions so they're referenceable from
    // any function, const, or impl body regardless of declaration order —
    // the value-level analogue of `collect_function_signatures`.
    collect_const_signatures(&mut infer, &module, &mut env);
    // Inherent method signatures register before any body is checked, so
    // methods are callable from every function and impl body regardless of
    // declaration order.
    register_inherent_impls(&mut infer, &mut module, &mut errors);

    // Phase 2: impl blocks. Inherent method bodies record their inferred
    // abilities for deferred enforcement (like functions, phase 4).
    let mut deferred_method_abilities: Vec<DeferredAbilityCheck> = Vec::new();
    check_impls(
        &mut infer,
        &mut module,
        &env,
        &mut errors,
        &mut deferred_method_abilities,
    );

    // Phase 3: function and const bodies.
    // Records (item index, raw inferred abilities) for deferred enforcement.
    let mut inferred_abilities: Vec<(usize, AbilitySet)> = Vec::new();

    for (idx, item) in module.items.iter_mut().enumerate() {
        if let crate::ast::ItemKind::Function(func) = &mut item.kind {
            infer.reset_abilities();

            let mut func_env = env.extend();
            let expected_ret_ty = func.ret_ty.clone().map(|ty| infer.resolve_holes(&ty));

            for param in &func.params {
                let param_ty = match &param.ty {
                    Some(ty) => infer.resolve_holes(ty),
                    None => infer.fresh(),
                };
                func_env.insert_mono(param.id, Arc::clone(&param.name), param_ty);
            }

            match infer.infer_expr(&func_env, &mut func.body) {
                Ok(body_ty) => {
                    if let Some(ref expected) = expected_ret_ty {
                        let span = (func.body.span.start, func.body.span.end);
                        if let Err(e) = infer.unify(expected, &body_ty, span) {
                            errors.push(e.with_context(format!(
                                "in function `{}`: return type mismatch",
                                func.name
                            )));
                        }
                    }

                    bind_inferred_abilities(&mut infer, &env, func);
                    inferred_abilities.push((idx, infer.current_abilities().clone()));
                }
                Err(e) => {
                    errors.push(e.with_context(format!("in function `{}`", func.name)));
                }
            }
        }

        if let crate::ast::ItemKind::Const(const_def) = &mut item.kind {
            check_const_body(&mut infer, &env, const_def, &mut errors);
        }
    }

    // Phase 4: enforce declared abilities with final substitutions applied.
    // Handle expressions whose body effects were polymorphic at the handle
    // site resolve first, so their remainders are concrete for enforcement;
    // deferred sandbox restrictions run on the resolved sets.
    infer.resolve_pending_discharges();
    infer.resolve_pending_sandbox_checks();
    for (idx, inferred) in inferred_abilities {
        if let crate::ast::ItemKind::Function(func) = &module.items[idx].kind {
            enforce_declared_abilities(
                &infer,
                func,
                module.items[idx].span,
                &inferred,
                &mut errors,
            );
        }
    }
    for check in &deferred_method_abilities {
        enforce_ability_subset(
            &infer,
            &check.context,
            &check.declared,
            &check.inferred,
            (check.span.start, check.span.end),
            &mut errors,
        );
    }

    errors.extend(infer.take_pending_errors());
    CheckResult { errors, module }
}

/// Register the cross-module context for a package build: platform dynamics,
/// foreign package items, and imports, returning the resulting import env.
///
/// Runs as the first half of Phase 1 when checking a module inside a
/// registry (as opposed to a standalone single-file check).
fn register_cross_module(
    infer: &mut Infer,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    errors: &mut Vec<BoxedTypeError>,
) -> TypeEnv {
    // Every module's abilities are always in scope fully-qualified. Seed
    // them as namespaced dynamics before imports and local abilities
    // resolve, so `platform::Stdio` / `pkg::effects::Counter` references
    // (inline uses and cross-module ability deps) and the
    // `use platform::Network;` bridge all find their target — on every
    // path that has a registry, including the package build.
    seed_namespaced_ability_dynamics(infer, registry, errors);
    // Imported enums register first: foreign impl registration (next)
    // must resolve an imported enum target to its uuid, or the impl's
    // dispatch key won't match the call sites'.
    register_imported_enums(infer, module_path, registry);
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

/// Bind an unannotated private function's ability variable to its body's
/// inferred effects, making the real effect set visible at call sites.
///
/// Annotated and public functions are skipped: their scheme carries the
/// declared (possibly empty) ability set, which enforcement checks instead.
fn bind_inferred_abilities(infer: &mut Infer, env: &TypeEnv, func: &crate::ast::FunctionDef) {
    if !func.abilities.is_empty() || func.is_public {
        return;
    }
    let Some(scheme) = env.get_by_name(&func.name) else {
        return;
    };
    let Type::Function(f) = &scheme.ty else {
        return;
    };
    let AbilitySet::Var(var_id) = f.abilities else {
        return;
    };

    // Call sites checked before this body may have unified the scheme's
    // variable with their own fresh variables (var → var links). Binding at
    // `var_id` directly would sever those links, leaving the callers'
    // variables dangling — follow the chain and bind its representative.
    let mut root = var_id;
    let mut seen = vec![var_id];
    while let Some(AbilitySet::Var(next)) = infer.ability_subst.get(&root) {
        if seen.contains(next) {
            break;
        }
        root = *next;
        seen.push(root);
    }

    let inferred = infer.apply_abilities(infer.current_abilities());

    // Self-recursion makes the body require the function's own variable:
    // `var = concrete ∪ var` solves to `var = concrete`.
    let bound = match inferred {
        AbilitySet::Var(id) if seen.contains(&id) => AbilitySet::Empty,
        AbilitySet::Row { concrete, tail } if seen.contains(&tail) => {
            AbilitySet::from_abilities(concrete)
        }
        other => other,
    };
    infer.ability_subst.insert(root, bound);
}

/// Verify that a function's inferred abilities are a subset of its declared
/// abilities.
///
/// Applies to annotated functions and to public functions (where no `with`
/// clause means "pure"). Unannotated private functions are skipped — their
/// abilities are inferred, not declared. Abilities that remain polymorphic
/// after substitution are not enforced; they are constrained at the call
/// sites that instantiate them.
fn enforce_declared_abilities(
    infer: &Infer,
    func: &crate::ast::FunctionDef,
    item_span: crate::ast::Span,
    inferred: &AbilitySet,
    errors: &mut Vec<BoxedTypeError>,
) {
    if func.abilities.is_empty() && !func.is_public {
        // Abilities were inferred (bind_inferred_abilities), nothing declared
        // to enforce against.
        return;
    }

    enforce_ability_subset(
        infer,
        &format!("function `{}`", func.name),
        &func.abilities,
        inferred,
        (item_span.start, item_span.end),
        errors,
    );
}

/// A recorded "body inferred these abilities against this declaration"
/// check, deferred until all bodies are checked (phase 4) so ability
/// variables bound late still resolve.
struct DeferredAbilityCheck {
    /// Human-readable owner for error context, e.g. "inherent method `map`".
    context: String,
    declared: Vec<crate::ast::QualifiedName>,
    inferred: AbilitySet,
    span: crate::ast::Span,
}

/// Verify that inferred abilities are a subset of the declared clause
/// (no clause means pure). Shared by function and inherent-method
/// enforcement.
fn enforce_ability_subset(
    infer: &Infer,
    context: &str,
    declared: &[crate::ast::QualifiedName],
    inferred: &AbilitySet,
    span: (u32, u32),
    errors: &mut Vec<BoxedTypeError>,
) {
    let inferred = infer.apply_abilities(inferred);

    // Namespace-aware resolution first (a `with platform::Stdio` clause
    // must mean the platform ability even when a local declaration
    // shadows the bare name), then a deliberately lenient bare fallback:
    // the namespace policy was already enforced where the clause was
    // resolved into the scheme (`build_function_scheme`,
    // `resolve_declared_abilities`), which reported
    // `AbilityRequiresNamespace` for a bare platform name. Resolving that
    // name leniently here keeps the reported error from cascading into a
    // second "uses ability but doesn't declare it" error.
    let declared: Vec<AbilityId> = declared
        .iter()
        .filter_map(|qn| {
            infer
                .ability_resolver
                .resolve_ref(&qn.resolved_module_segments(), qn.resolved_name())
                .ok()
                .or_else(|| infer.ability_name_to_id(&qn.name))
        })
        .collect();
    let declared_set = AbilitySet::from_abilities(declared);

    let inferred_ids = match &inferred {
        AbilitySet::Concrete(ids) => ids.as_slice(),
        AbilitySet::Row { concrete, .. } => concrete.as_slice(),
        AbilitySet::Empty | AbilitySet::Var(_) | AbilitySet::Unresolved(_) => &[],
    };

    for ability_id in inferred_ids {
        if !declared_set.contains(*ability_id) {
            let name = infer
                .ability_id_to_name(*ability_id)
                .unwrap_or("<unknown>")
                .to_string();
            errors.push(Box::new(
                TypeError::new(
                    TypeErrorKind::MissingAbility {
                        required: *ability_id,
                        available: declared_set.clone(),
                    },
                    span,
                )
                .with_context(format!(
                    "{context} uses ability `{name}` but doesn't declare it"
                )),
            ));
        }
    }
}

/// The `(name, type, is_public)` view shared by the two named-type items:
/// `struct` definitions and `type` aliases. Both register the same way — a name
/// resolving to a type in the inferencer's substitution table. For a non-`unique`
/// struct that type is a bare record, so it substitutes structurally exactly
/// like an alias; `unique` structs carry a `Type::Nominal` identity instead.
fn named_type_def(item: &crate::ast::Item) -> Option<(&Arc<str>, &Type, bool)> {
    match &item.kind {
        crate::ast::ItemKind::Struct(s) => Some((&s.name, &s.ty, s.is_public)),
        crate::ast::ItemKind::TypeAlias(t) => Some((&t.name, &t.ty, t.is_public)),
        _ => None,
    }
}

/// Register all struct definitions and type aliases from a module into the
/// inferencer so their names resolve as types while checking.
fn register_named_types(infer: &mut Infer, module: &crate::ast::Module) {
    for item in &module.items {
        if let Some((name, ty, _)) = named_type_def(item) {
            infer.register_type_alias(Arc::clone(name), ty.clone());
        }
    }
}

/// Bind each local unit struct as a value in the type env, mirroring
/// `register_enums`' nullary-variant binding: a unit struct is both a type
/// and its unique value, so a bare `Origin` in value position type-checks
/// to the struct's nominal type. `struct.ty` is already the `Type::Nominal`,
/// so nominal identity rides along exactly like a nullary variant
/// constructor's scheme. Only unit structs get the value binding; a
/// field-bearing struct used bare still fails as an undefined value.
fn register_unit_struct_values(module: &crate::ast::Module, env: &mut TypeEnv) {
    // Synthetic binding ids distinct from imports (2_000_000+) and enum
    // variant constructors (4_000_000+).
    let mut next_binding_id: BindingId = 5_000_000;
    for item in &module.items {
        if let crate::ast::ItemKind::Struct(s) = &item.kind
            && s.is_unit_value()
        {
            env.insert(
                next_binding_id,
                Arc::clone(&s.name),
                Scheme::mono(s.ty.clone()),
            );
            next_binding_id += 1;
        }
    }
}

/// Register all trait definitions from a module into the trait registry.
fn register_traits(infer: &mut Infer, module: &crate::ast::Module) {
    for item in &module.items {
        if let crate::ast::ItemKind::Trait(trait_def) = &item.kind {
            register_trait_def(infer, trait_def);
        }
    }
}

/// Register a single trait definition into the trait registry.
fn register_trait_def(infer: &mut Infer, trait_def: &crate::ast::TraitDef) {
    let trait_id = infer.trait_registry.fresh_id();

    // Build method definitions
    let methods: Vec<TraitMethodDef> = trait_def
        .methods
        .iter()
        .map(|m| {
            TraitMethodDef::new(
                Arc::clone(&m.name),
                m.has_self,
                m.params.iter().map(|(_, ty)| ty.clone()).collect(),
                m.ret_ty.clone(),
            )
        })
        .collect();

    // Create and register the trait definition
    let def = TraitDef {
        id: trait_id,
        name: Arc::clone(&trait_def.name),
        type_params: Vec::new(), // TODO: Handle type params properly
        methods,
        supertraits: Vec::new(), // TODO: Resolve supertrait references
    };

    infer.trait_registry.register_trait(def);
}

/// Register the enums a module imports (`use pkg::m::{SomeEnum}`) into the
/// enum registry, as if they were declared locally: the type name resolves,
/// and `register_enums` later binds their variant constructors and patterns
/// alongside the local ones. Local declarations register afterwards, so
/// they shadow imported variants — the same precedence the compiler applies.
fn register_imported_enums(
    infer: &mut Infer,
    current_module: &ModulePath,
    registry: &ModuleRegistry,
) {
    let Ok(resolved) = registry.resolve_imports(current_module) else {
        return;
    };
    for (name, bindings) in resolved.imports {
        for import in bindings {
            let ResolvedImport::Symbol {
                from_module,
                export_kind: ExportKind::Enum,
                ..
            } = import
            else {
                continue;
            };
            if let Some(module_info) = registry.get(&from_module) {
                for item in &module_info.module.items {
                    if let crate::ast::ItemKind::Enum(def) = &item.kind
                        && def.name == name
                    {
                        infer.enum_registry.register_def(def);
                    }
                }
            }
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
    // Canonical qualified keys (`shapes.Money`) always stay: they can't
    // collide with bare names, and qualified references are visibility-
    // checked by the resolve pass.
    infer.retain_type_aliases(|name| name.contains("::") || imported.contains(name));
}

/// Register the types, traits, and impls declared in the *other* modules of
/// the package so they can be resolved while checking this module.
///
/// Foreign items are registered by signature only — their bodies were (or
/// will be) checked in their own module's check pass. Impls register the
/// dispatch mapping `(trait, type uuid) → method symbol`; the symbols are
/// resolved to content hashes at link time like any function name.
///
/// Traits and impls stay build-global for coherence, but the foreign *type
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

    // Types and traits first: impl registration needs both resolvable.
    for info in &foreign_modules {
        register_named_types(infer, &info.module);
        register_traits(infer, &info.module);
        // Public named types also register under their canonical qualified key
        // (`shapes.Money`): the resolve pass rewrites qualified type
        // constructors (`pkg::shapes::Money { … }`) to that key, and canonical
        // keys are never retracted (they can't leak as bare names).
        for item in &info.module.items {
            if let Some((name, ty, true)) = named_type_def(item) {
                let key: Arc<str> = format!("{}::{}", info.path, name).into();
                infer.register_type_alias(key, ty.clone());
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
    let Some(trait_id) = infer.trait_registry.lookup_trait(&trait_name.name) else {
        return;
    };
    let for_type = infer.resolve_holes(&impl_def.for_type);
    let Type::Nominal(nominal_type) = &for_type else {
        return;
    };

    let mut impl_record = crate::types::TraitImpl::new(trait_id, nominal_type.clone());
    for method in &impl_def.methods {
        let symbol =
            crate::types::impl_method_symbol(&nominal_type.uuid, &trait_name.name, &method.name);
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

/// Check impl blocks and register implementations.
fn check_impls(
    infer: &mut Infer,
    module: &mut crate::ast::Module,
    env: &TypeEnv,
    errors: &mut Vec<BoxedTypeError>,
    deferred: &mut Vec<DeferredAbilityCheck>,
) {
    for item in &mut module.items {
        if let crate::ast::ItemKind::Impl(impl_def) = &mut item.kind {
            match impl_def.trait_name.clone() {
                Some(trait_name) => {
                    check_single_impl(
                        infer,
                        impl_def,
                        &trait_name,
                        item.span,
                        env,
                        errors,
                        deferred,
                    );
                }
                None => check_inherent_impl_bodies(infer, impl_def, env, errors, deferred),
            }
        }
    }
}

/// Check a single trait impl block.
fn check_single_impl(
    infer: &mut Infer,
    impl_def: &mut crate::ast::ImplDef,
    trait_name: &crate::ast::QualifiedName,
    item_span: crate::ast::Span,
    env: &TypeEnv,
    errors: &mut Vec<BoxedTypeError>,
    deferred: &mut Vec<DeferredAbilityCheck>,
) {
    let span = (item_span.start, item_span.end);

    // Look up the trait
    let Some(trait_id) = infer.trait_registry.lookup_trait(&trait_name.name) else {
        errors.push(Box::new(TypeError::new(
            TypeErrorKind::UnknownTrait {
                name: Arc::clone(&trait_name.name),
            },
            span,
        )));
        return;
    };

    // Verify the implementing type is nominal
    let for_type = infer.resolve_holes(&impl_def.for_type);
    let Type::Nominal(nominal_type) = &for_type else {
        errors.push(Box::new(TypeError::new(
            TypeErrorKind::TraitOnStructuralType {
                trait_name: Arc::clone(&trait_name.name),
                ty: for_type.clone(),
            },
            span,
        )));
        return;
    };

    // Get the trait definition to check method signatures
    let Some(trait_def) = infer.trait_registry.get_trait(trait_id).cloned() else {
        return;
    };

    // Check each method in the impl
    check_impl_methods(
        infer, impl_def, &trait_def, &for_type, env, errors, deferred,
    );

    // Check that all required methods are implemented
    check_impl_completeness(impl_def, &trait_def, trait_name, span, errors);

    // Register the impl, assigning each method its canonical function symbol.
    // The compiler registers method bodies under these symbols so they are
    // content-addressed like ordinary functions; call sites resolve the
    // symbol through the same name→hash table as regular calls.
    let mut impl_record = crate::types::TraitImpl::new(trait_id, nominal_type.clone());
    for method in &mut impl_def.methods {
        let symbol =
            crate::types::impl_method_symbol(&nominal_type.uuid, &trait_name.name, &method.name);
        impl_record
            .methods
            .insert(Arc::clone(&method.name), Arc::clone(&symbol));
        method.resolved_symbol = Some(symbol);
    }
    if infer.trait_registry.register_impl(impl_record).is_some() {
        errors.push(Box::new(TypeError::new(
            TypeErrorKind::DuplicateImpl {
                trait_name: Arc::clone(&trait_name.name),
                ty: for_type.clone(),
            },
            span,
        )));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Inherent impls
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve an inherent impl's target type to its coherence key.
///
/// Returns `None` when the target cannot carry inherent methods: a
/// structural type (record, tuple, function) or a bare impl type parameter
/// (which would be a blanket impl).
fn inherent_impl_target(
    infer: &mut Infer,
    impl_def: &crate::ast::ImplDef,
) -> Option<(inherent::ImplKey, Type)> {
    let for_type = infer.resolve_holes(&impl_def.for_type);

    // `impl<T> T` — a blanket impl over every type — is not a thing.
    if let Type::Named(n) = &for_type
        && n.args.is_empty()
        && impl_def
            .type_params
            .iter()
            .any(|tp| tp.name.as_ref() == n.name.as_ref())
    {
        return None;
    }

    let key = inherent::impl_key_for(&for_type)?;
    Some((key, for_type))
}

/// Register inherent impl signatures and assign dispatch symbols.
///
/// Runs before any body checking so inherent methods resolve from every
/// function and impl body regardless of declaration order. Coherence is
/// enforced here: a second definition of the same method name for the same
/// target type is an error, because both would claim one dispatch symbol.
fn register_inherent_impls(
    infer: &mut Infer,
    module: &mut crate::ast::Module,
    errors: &mut Vec<BoxedTypeError>,
) {
    for item in &mut module.items {
        let crate::ast::ItemKind::Impl(impl_def) = &mut item.kind else {
            continue;
        };
        if impl_def.trait_name.is_some() {
            continue;
        }
        let span = (item.span.start, item.span.end);

        let target = inherent_impl_target(infer, impl_def);
        // Beyond being keyable, a named target must actually exist: a
        // built-in primitive (`String`, `Number`, ...), a declared enum, or
        // one of the built-in containers. (Nominal types were already
        // resolved through their alias.)
        let target = target.filter(|(_, for_type)| match for_type {
            Type::Named(n) => {
                n.uuid
                    .and_then(crate::types::Primitive::from_uuid)
                    .is_some()
                    || infer.enum_registry.get(&n.name).is_some()
                    || matches!(n.name.as_ref(), "List" | "Map" | "Set")
            }
            _ => true,
        });
        let Some((key, for_type)) = target else {
            errors.push(Box::new(TypeError::new(
                TypeErrorKind::InherentImplInvalidTarget {
                    ty: infer.resolve_holes(&impl_def.for_type),
                },
                span,
            )));
            continue;
        };

        let impl_type_params = impl_def.type_params.clone();
        for method in &mut impl_def.methods {
            let scheme =
                build_inherent_method_scheme(infer, &impl_type_params, method, &for_type, errors);
            let symbol = inherent::inherent_method_symbol(&key, &method.name);
            let record = inherent::InherentMethod {
                name: Arc::clone(&method.name),
                has_self: method.has_self,
                scheme,
                symbol: Arc::clone(&symbol),
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
                    (method.span.start, method.span.end),
                )));
            }
            method.resolved_symbol = Some(symbol);
        }
    }
}

/// Build the callable scheme for an inherent method: the full function type
/// `(self, params...) -> ret with abilities`, quantified over the impl's
/// and the method's type parameters. Call sites instantiate it exactly like
/// a generic function's scheme.
fn build_inherent_method_scheme(
    infer: &mut Infer,
    impl_type_params: &[crate::ast::TypeParam],
    method: &crate::ast::ImplMethod,
    for_type: &Type,
    errors: &mut Vec<BoxedTypeError>,
) -> Scheme {
    // Quantified ids come from the shared generator so they can never
    // collide with inference variables allocated elsewhere.
    let mut type_var_map: HashMap<Arc<str>, TypeVarId> = HashMap::new();
    let mut quantified = Vec::new();
    for tp in impl_type_params.iter().chain(method.type_params.iter()) {
        let var_id = infer.r#gen.fresh_id();
        type_var_map.insert(Arc::clone(&tp.name), var_id);
        quantified.push(var_id);
    }

    let self_ty = substitute_type_params(for_type, &type_var_map);

    let mut params = Vec::new();
    if method.has_self {
        params.push(self_ty.clone());
    }
    for p in &method.params {
        let ty = match &p.ty {
            Some(ty) => resolve_signature_type(infer, ty, for_type, &type_var_map),
            None => infer.fresh(),
        };
        params.push(ty);
    }

    let ret = if let Some(ty) = &method.ret_ty {
        resolve_signature_type(infer, ty, for_type, &type_var_map)
    } else {
        // The signature is the dispatch contract; without a declared
        // return type foreign callers would see a dangling variable.
        errors.push(Box::new(TypeError::new(
            TypeErrorKind::InherentMethodMissingReturnType {
                method: Arc::clone(&method.name),
            },
            (method.span.start, method.span.end),
        )));
        infer.fresh()
    };

    let abilities = resolve_declared_abilities(infer, &method.abilities, method.span, errors);
    let fn_ty = Type::function_with_abilities(params, ret, abilities);
    if quantified.is_empty() {
        Scheme::mono(fn_ty)
    } else {
        Scheme::poly(quantified, fn_ty)
    }
}

/// Resolve a declared type from an inherent method signature: substitute
/// `Self`, then the quantified type parameters, then expand aliases/holes.
fn resolve_signature_type(
    infer: &mut Infer,
    ty: &Type,
    for_type: &Type,
    type_var_map: &HashMap<Arc<str>, TypeVarId>,
) -> Type {
    let ty = substitute_self(ty, for_type);
    let ty = substitute_type_params(&ty, type_var_map);
    infer.resolve_holes(&ty)
}

/// Resolve a `with` clause to a concrete ability set.
///
/// Unknown names go into the caller's error sink, not the shared pending
/// list: foreign signature registration passes a scratch vec so a foreign
/// module's mistakes (or abilities that only resolve in its own context)
/// don't surface as errors of the module currently being checked.
fn resolve_declared_abilities(
    infer: &mut Infer,
    declared: &[crate::ast::QualifiedName],
    span: crate::ast::Span,
    errors: &mut Vec<BoxedTypeError>,
) -> AbilitySet {
    let mut ids = Vec::new();
    for qn in declared {
        match infer.resolve_ability_ref(qn, (span.start, span.end)) {
            Ok(id) => ids.push(id),
            Err(e) => errors.push(e),
        }
    }
    AbilitySet::from_abilities(ids)
}

/// Type-check the bodies of an inherent impl block.
///
/// Type parameters stay opaque (`Named("T")`) inside bodies — rigid, like
/// generic function bodies. Each body's inferred abilities are recorded for
/// deferred enforcement against the method's `with` clause (no clause means
/// pure, like a public function).
fn check_inherent_impl_bodies(
    infer: &mut Infer,
    impl_def: &mut crate::ast::ImplDef,
    env: &TypeEnv,
    errors: &mut Vec<BoxedTypeError>,
    deferred: &mut Vec<DeferredAbilityCheck>,
) {
    let for_type = infer.resolve_holes(&impl_def.for_type);
    for method in &mut impl_def.methods {
        if method.resolved_symbol.is_none() {
            // Registration rejected the whole impl (invalid target); the
            // error is already reported.
            continue;
        }

        infer.reset_abilities();
        let mut func_env = env.extend();

        if method.has_self {
            func_env.insert_mono(method.self_id, Arc::from("self"), for_type.clone());
        }
        for param in &method.params {
            let param_ty = match &param.ty {
                Some(ty) => {
                    let ty = substitute_self(ty, &for_type);
                    infer.resolve_holes(&ty)
                }
                None => infer.fresh(),
            };
            func_env.insert_mono(param.id, Arc::clone(&param.name), param_ty);
        }

        let expected_ret = method.ret_ty.as_ref().map(|ty| {
            let ty = substitute_self(ty, &for_type);
            infer.resolve_holes(&ty)
        });

        match infer.infer_expr(&func_env, &mut method.body) {
            Ok(body_ty) => {
                if let Some(expected) = &expected_ret {
                    let method_span = (method.span.start, method.span.end);
                    if let Err(e) = infer.unify(expected, &body_ty, method_span) {
                        errors
                            .push(e.with_context(format!("in inherent method `{}`", method.name)));
                    }
                }
                deferred.push(DeferredAbilityCheck {
                    context: format!("inherent method `{}`", method.name),
                    declared: method.abilities.clone(),
                    inferred: infer.current_abilities().clone(),
                    span: method.span,
                });
            }
            Err(e) => {
                errors.push(e.with_context(format!("in inherent method `{}`", method.name)));
            }
        }
    }
}

/// Check all methods in an impl block.
///
/// Trait method signatures carry no `with` clause, so trait impl bodies
/// must be pure — enforced like a public function's empty declaration,
/// deferred until all bodies are checked. Without this, dot dispatch and
/// operator overloading would launder arbitrary effects past callers.
fn check_impl_methods(
    infer: &mut Infer,
    impl_def: &mut crate::ast::ImplDef,
    trait_def: &TraitDef,
    for_type: &Type,
    env: &TypeEnv,
    errors: &mut Vec<BoxedTypeError>,
    deferred: &mut Vec<DeferredAbilityCheck>,
) {
    for method in &mut impl_def.methods {
        // Trait method effects are fixed by the trait signature; a `with`
        // clause on the impl method has nothing to attach to.
        if !method.abilities.is_empty() {
            errors.push(Box::new(TypeError::new(
                TypeErrorKind::AbilityClauseOnTraitImpl {
                    method: Arc::clone(&method.name),
                },
                (method.span.start, method.span.end),
            )));
        }

        let trait_method = trait_def
            .methods
            .iter()
            .find(|m| m.name.as_ref() == method.name.as_ref());

        let Some(tm) = trait_method else {
            errors.push(Box::new(TypeError::new(
                TypeErrorKind::MethodNotFound {
                    method: Arc::clone(&method.name),
                    ty: Type::Named(crate::types::NamedType::simple(Arc::clone(&trait_def.name))),
                },
                (method.span.start, method.span.end),
            )));
            continue;
        };

        // Type-check the method body
        infer.reset_abilities();
        let mut func_env = env.extend();

        // Add self parameter
        if tm.has_self {
            func_env.insert_mono(method.self_id, Arc::from("self"), for_type.clone());
        }

        // Add other parameters, substituting Self with the implementing type
        for (param, expected_ty) in method.params.iter().zip(tm.params.iter()) {
            // Substitute Self with for_type in the expected type from trait method
            let expected_ty_substituted = substitute_self(expected_ty, for_type);
            let param_ty = param.ty.as_ref().map_or_else(
                || expected_ty_substituted.clone(),
                |ty| infer.resolve_holes(ty),
            );
            func_env.insert_mono(param.id, Arc::clone(&param.name), param_ty);
        }

        // Infer body type and check against expected return type
        // Substitute Self with for_type in the expected return type
        let expected_ret = substitute_self(&tm.ret, for_type);
        match infer.infer_expr(&func_env, &mut method.body) {
            Ok(body_ty) => {
                let method_span = (method.span.start, method.span.end);
                if let Err(e) = infer.unify(&expected_ret, &body_ty, method_span) {
                    errors.push(e.with_context(format!("in impl method `{}`", method.name)));
                }
                deferred.push(DeferredAbilityCheck {
                    context: format!("trait impl method `{}`", method.name),
                    declared: Vec::new(),
                    inferred: infer.current_abilities().clone(),
                    span: method.span,
                });
            }
            Err(e) => {
                errors.push(e.with_context(format!("in impl method `{}`", method.name)));
            }
        }
    }
}

/// Check that all required trait methods are implemented.
fn check_impl_completeness(
    impl_def: &crate::ast::ImplDef,
    trait_def: &TraitDef,
    trait_name: &crate::ast::QualifiedName,
    span: (u32, u32),
    errors: &mut Vec<BoxedTypeError>,
) {
    for trait_method in &trait_def.methods {
        let implemented = impl_def
            .methods
            .iter()
            .any(|m| m.name.as_ref() == trait_method.name.as_ref());
        if !implemented {
            errors.push(Box::new(TypeError::new(
                TypeErrorKind::ImplMissingMethod {
                    trait_name: Arc::clone(&trait_name.name),
                    method: Arc::clone(&trait_method.name),
                },
                span,
            )));
        }
    }
}

/// Collect function signatures into the environment.
fn collect_function_signatures(infer: &mut Infer, module: &crate::ast::Module, env: &mut TypeEnv) {
    let mut next_binding_id: BindingId = 1_000_000;
    for item in &module.items {
        if let crate::ast::ItemKind::Function(func) = &item.kind {
            let binding_id = next_binding_id;
            next_binding_id += 1;
            let scheme = build_function_scheme(infer, func, true);
            env.insert(binding_id, Arc::clone(&func.name), scheme);
        }
    }
}

/// Register module-level `const` declarations into the environment.
///
/// Runs in Phase 1 (before any body is checked) so a constant is in scope
/// for every function/const/impl body irrespective of source order — the
/// value-level counterpart to [`collect_function_signatures`]. Only the
/// declared type is registered here; the value expression is type-checked
/// against that annotation later in Phase 3. Aliases and holes in the
/// annotation are resolved so the registered scheme matches the type uses
/// unify against (mirroring the Phase 3 `resolve_holes` on the same type).
fn collect_const_signatures(infer: &mut Infer, module: &crate::ast::Module, env: &mut TypeEnv) {
    let mut next_binding_id: BindingId = 2_000_000;
    for item in &module.items {
        if let crate::ast::ItemKind::Const(const_def) = &item.kind {
            let binding_id = next_binding_id;
            next_binding_id += 1;
            let ty = infer.resolve_holes(&const_def.ty);
            env.insert(binding_id, Arc::clone(&const_def.name), Scheme::mono(ty));
        }
    }
}

/// Check one `const` body: enforce that the initializer is a literal and
/// that its type matches the annotation.
fn check_const_body(
    infer: &mut Infer,
    env: &TypeEnv,
    const_def: &mut crate::ast::ConstDef,
    errors: &mut Vec<BoxedTypeError>,
) {
    infer.reset_abilities();

    // A `const` maps an identifier to a single hashed primitive value, so its
    // initializer must be a literal — not an identifier, call, or compound
    // expression. `const_eval` is the shared authority on what qualifies; the
    // compiler inlines exactly this set.
    if crate::const_eval::literal_value(&const_def.value).is_none() {
        errors.push(Box::new(TypeError::new(
            TypeErrorKind::ConstNotLiteral {
                name: Arc::clone(&const_def.name),
            },
            (const_def.value.span.start, const_def.value.span.end),
        )));
    }

    let expected_ty = infer.resolve_holes(&const_def.ty);

    match infer.infer_expr(env, &mut const_def.value) {
        Ok(actual_ty) => {
            let span = (const_def.value.span.start, const_def.value.span.end);
            if let Err(e) = infer.unify(&expected_ty, &actual_ty, span) {
                errors.push(
                    e.with_context(format!("in constant `{}`: type mismatch", const_def.name)),
                );
            }
        }
        Err(e) => {
            errors.push(e.with_context(format!("in constant `{}`", const_def.name)));
        }
    }
}

/// Build a type scheme for a function from its signature.
///
/// `infer_abilities` controls what an absent `with` clause means: for local
/// private functions (true) the scheme gets a fresh ability variable that
/// [`bind_inferred_abilities`] later binds to the body's inferred effects;
/// for public or foreign functions (false) it means "pure".
fn build_function_scheme(
    infer: &mut Infer,
    func: &crate::ast::FunctionDef,
    infer_abilities: bool,
) -> Scheme {
    // Collect type variables from type parameters
    let mut type_var_map: HashMap<Arc<str>, TypeVarId> = HashMap::new();
    let mut quantified_vars = Vec::new();

    for tp in &func.type_params {
        // Quantified ids come from the shared generator so they can never
        // collide with inference variables allocated elsewhere (a low fixed
        // id like 0 would alias the first `fresh()` of the check pass).
        let var_id = infer.r#gen.fresh_id();
        type_var_map.insert(Arc::clone(&tp.name), var_id);
        quantified_vars.push(var_id);
    }

    // Build parameter types, resolving type aliases
    let param_types: Vec<Type> = func
        .params
        .iter()
        .map(|p| match &p.ty {
            Some(ty) => {
                let substituted = substitute_type_params(ty, &type_var_map);
                infer.resolve_holes(&substituted)
            }
            None => infer.fresh(),
        })
        .collect();

    // Build return type, resolving type aliases
    let ret_ty = match &func.ret_ty {
        Some(ty) => {
            let substituted = substitute_type_params(ty, &type_var_map);
            infer.resolve_holes(&substituted)
        }
        None => infer.fresh(),
    };

    // Build ability set from declared abilities. Unknown names are
    // reported (via pending errors) rather than silently dropped — a typo
    // in a `with` clause must not quietly declare the function pure.
    // Foreign signatures (`infer_abilities` false via `get_symbol_scheme`)
    // report too: the name resolves against this module's resolver, and a
    // missing ability is equally an error at the import site.
    let abilities = if func.abilities.is_empty() {
        if infer_abilities && !func.is_public {
            infer.fresh_ability_var()
        } else {
            AbilitySet::Empty
        }
    } else {
        let mut ability_ids: Vec<AbilityId> = Vec::with_capacity(func.abilities.len());
        for qn in &func.abilities {
            match infer.resolve_ability_ref(qn, (0, 0)) {
                Ok(id) => ability_ids.push(id),
                Err(e) => infer
                    .pending_errors
                    .push(e.with_context(format!("in `with` clause of function `{}`", func.name))),
            }
        }
        AbilitySet::from_abilities(ability_ids)
    };

    let fn_ty = Type::function_with_abilities(param_types, ret_ty, abilities);

    if quantified_vars.is_empty() {
        Scheme::mono(fn_ty)
    } else {
        Scheme::poly(quantified_vars, fn_ty)
    }
}

/// Register the module's `ability` declarations.
///
/// Each declaration's method signatures are resolved (type parameters
/// become quantified type variables, aliases expand), rendered to
/// canonical form, and hashed into the ability's content-addressed
/// identity. The resulting [`DynAbility`] joins the resolver so
/// perform/suspend/handle and `with` clauses see it exactly like a
/// builtin; the computed identity is stored back into the AST for the
/// compiler.
///
/// Declared dependencies (a local `ability B with A`) resolve against
/// abilities already known to the resolver — builtins or dynamics
/// registered earlier in the item list — and are recorded in the ability
/// registry so requiring the ability transitively requires them.
fn register_abilities(
    infer: &mut Infer,
    module: &mut crate::ast::Module,
    errors: &mut Vec<BoxedTypeError>,
) -> Vec<Arc<crate::ability_resolver::DynAbility>> {
    let mut resolved = Vec::new();
    for item in &mut module.items {
        let crate::ast::ItemKind::Ability(def) = &mut item.kind else {
            continue;
        };

        let dyn_ab = resolve_ability_def(infer, def, errors);
        // The compiler reads the identity back from the AST.
        def.resolved_id = Some(dyn_ab.id);
        infer.ability_resolver.register_dynamic(dyn_ab);
        if let Some(ability) = infer.ability_resolver.get_dynamic(&def.name) {
            resolved.push(Arc::clone(ability));
        }
    }
    resolved
}

/// Register a cross-module ability import (`use pkg::b::SomeAbility;`,
/// `use platform::Network;`) as a *bare* local dynamic, resolved from the
/// origin module's declaration.
///
/// The identity is content-addressed, so it unifies with the origin
/// module's own registration — and with any namespaced copy
/// (`platform::Network`) — meaning handlers, effect-rows, and linking need
/// no changes. Called from `build_import_env` for each `ExportKind::Ability`
/// import.
fn register_imported_ability(
    infer: &mut Infer,
    registry: &ModuleRegistry,
    from_module: &ModulePath,
    name: &str,
    errors: &mut Vec<BoxedTypeError>,
) {
    let Some(module_info) = registry.get(from_module) else {
        return;
    };
    let Some(def) = module_info
        .module
        .items
        .iter()
        .find_map(|item| match &item.kind {
            crate::ast::ItemKind::Ability(def) if def.name.as_ref() == name => Some(def),
            _ => None,
        })
    else {
        return;
    };
    let dyn_ab = resolve_ability_def(infer, def, errors);
    infer.ability_resolver.register_dynamic(dyn_ab);
}

/// Seed every registered module's `ability` declarations as namespaced
/// dynamics under the declaring module's dotted path (`platform.Network`,
/// `effects.Counter`, `deep.nested.fx.Log`).
///
/// This is the ability-layer counterpart of canonical name resolution:
/// the resolve pass rewrites every qualified or imported ability
/// reference to `<declaring module>::<Ability>`, and this seeding is what
/// makes that namespace resolvable — on every checking path that has a
/// registry (single-file, package, and LSP). Because ability identity is
/// the content-addressed interface hash, seeding is deterministic and a
/// bare local registration of the same declaration unifies with it.
///
/// Every module seeds — including the current one, whose declarations
/// *also* register bare in `register_abilities` (locals stay bare;
/// references to them normalize to the bare form). Seeding the current
/// module's namespace matters for hydrating foreign signatures: checking
/// `effects` hydrates `worker.tick`, whose `with` clause resolved to
/// `effects::Counter`. The `platform` module seeds first so its
/// intra-file dependencies (`Log with platform::Stdio`) resolve; other
/// modules seed in path order. Resolution errors inside *foreign* modules
/// are not this module's diagnostics — they surface when that module
/// itself is checked — except for `platform`, whose declarations have no
/// other checking path.
fn seed_namespaced_ability_dynamics(
    infer: &mut Infer,
    registry: &ModuleRegistry,
    errors: &mut Vec<BoxedTypeError>,
) {
    let mut modules: Vec<_> = registry
        .all_modules()
        .map(|info| (info.path.clone(), Arc::clone(&info.module)))
        .collect();
    modules.sort_by_key(|(path, _)| {
        // Platform first, then path order.
        (
            path.segments().first().map(AsRef::as_ref) != Some("platform"),
            path.to_string(),
        )
    });

    for (path, module) in modules {
        let is_platform = path.segments().first().map(AsRef::as_ref) == Some("platform");
        let namespace = path.to_string();
        let mut foreign_errors = Vec::new();
        for item in &module.items {
            if let crate::ast::ItemKind::Ability(def) = &item.kind {
                let dyn_ab = resolve_ability_def(infer, def, &mut foreign_errors);
                infer
                    .ability_resolver
                    .register_dynamic_in_namespace(&namespace, dyn_ab);
            }
        }
        if is_platform {
            errors.append(&mut foreign_errors);
        }
    }
}

/// Resolve one `ability` declaration into a content-addressed
/// [`DynAbility`], recording its transitive dependencies in the ability
/// registry.
///
/// Shared by the local path ([`register_abilities`], which additionally
/// writes the identity back into the AST and registers it *bare*) and the
/// cross-module import path ([`build_import_env`], which registers the
/// result bare from a foreign module's declaration). The identity is
/// recomputed deterministically from the canonical interface, so a foreign
/// import matches the origin module's own registration without touching the
/// (immutable) foreign AST's `resolved_id`.
fn resolve_ability_def(
    infer: &mut Infer,
    def: &crate::ast::AbilityDef,
    errors: &mut Vec<BoxedTypeError>,
) -> crate::ability_resolver::DynAbility {
    use crate::ability_resolver::{CanonicalTypeRenderer, DynAbility, DynMethod};

    // Resolve dependencies first: they must already be known. The
    // namespace policy applies here too: `ability Log with
    // platform::Stdio` — a platform dependency needs its prefix.
    let mut dependencies = Vec::new();
    for dep in &def.dependencies {
        match infer.resolve_ability_ref(dep, (def.name_span.start, def.name_span.end)) {
            Ok(id) => dependencies.push(id),
            Err(e) => errors.push(e),
        }
    }

    let mut methods = Vec::new();
    let mut canonical = Vec::new();
    #[allow(clippy::cast_possible_truncation)]
    for (idx, method) in def.methods.iter().enumerate() {
        // Type parameters become quantified variables, substituted
        // into the declared types.
        let mut param_map = HashMap::new();
        let mut quantified = Vec::new();
        for tp in &method.type_params {
            let var_id = infer.r#gen.fresh_id();
            param_map.insert(Arc::clone(&tp.name), var_id);
            quantified.push(var_id);
        }

        let params: Vec<Type> = method
            .params
            .iter()
            .map(|(_, ty)| infer.resolve_holes(&substitute_type_params(ty, &param_map)))
            .collect();
        let ret = infer.resolve_holes(&substitute_type_params(&method.ret_ty, &param_map));

        // One renderer per signature: variable numbering is
        // signature-local, by first occurrence.
        let mut renderer = CanonicalTypeRenderer::new();
        let canon_params: Vec<String> = params.iter().map(|p| renderer.render(p)).collect();
        let canon_ret = renderer.render(&ret);
        canonical.push((Arc::clone(&method.name), canon_params, canon_ret));

        methods.push(DynMethod {
            id: idx as u16,
            name: Arc::clone(&method.name),
            param_names: method.params.iter().map(|(n, _)| Arc::clone(n)).collect(),
            params,
            ret,
            quantified,
        });
    }

    let id = DynAbility::hash_from_canonical(&def.name, &canonical);

    // Record dependencies so `require_ability` pulls them transitively.
    if !dependencies.is_empty() {
        let registry = infer
            .ability_registry
            .get_or_insert_with(crate::types::AbilityRegistry::new);
        let mut info = crate::types::AbilityInfo::new(def.name.as_ref());
        for dep in &dependencies {
            info = info.with_dependency(*dep);
        }
        registry.register(id, info);
    }

    DynAbility {
        id,
        name: Arc::clone(&def.name),
        methods,
        dependencies,
    }
}

/// Resolve a module's `ability` declarations without checking the rest of
/// the module.
///
/// This is the entry point for **ability preludes**: an embedder parses a
/// module containing only `ability` declarations (e.g. the platform
/// bindings interface), resolves them here, and registers the results as
/// namespaced dynamics on the resolver it threads into checking — and as
/// the identity/method-id source when binding host handlers on the VM.
///
/// Each declaration's `resolved_id` is written back into the AST, exactly
/// as during a full module check.
pub fn resolve_ability_declarations(
    module: &mut crate::ast::Module,
) -> (
    Vec<Arc<crate::ability_resolver::DynAbility>>,
    Vec<BoxedTypeError>,
) {
    let mut infer = Infer::new();
    let mut errors = Vec::new();

    // Register each declaration under the reserved `platform` namespace
    // *before* resolving the next, so an intra-module dependency
    // (`ability Log with platform::Stdio`) resolves exactly as it does when
    // checking user code (see `seed_namespaced_platform_dynamics`, which
    // also hardcodes `platform`). Registering these bare — as the local
    // module path does — would leave a `platform::`-qualified dependency
    // unresolvable.
    let mut abilities = Vec::new();
    for item in &mut module.items {
        let crate::ast::ItemKind::Ability(def) = &mut item.kind else {
            continue;
        };
        let dyn_ab = resolve_ability_def(&mut infer, def, &mut errors);
        // The compiler reads the identity back from the AST.
        def.resolved_id = Some(dyn_ab.id);
        infer
            .ability_resolver
            .register_dynamic_in_namespace("platform", dyn_ab);
        if let Some(ability) = infer.ability_resolver.get_namespaced("platform", &def.name) {
            abilities.push(Arc::clone(ability));
        }
    }
    (abilities, errors)
}

/// Resolve every registered module's `ability` declarations to their
/// content-addressed identities, keyed canonically
/// (`<module path>::<Ability>`).
///
/// The build hands this to the compiler as its foreign-ability channel:
/// performs and handler arms that the resolve pass canonicalized to a
/// foreign module need the interface identity and method order, which no
/// name→hash table carries. Identity is deterministic (the interface
/// hash), so recomputing here matches the declaring module's own
/// registration exactly. Cross-module dependency resolution failures are
/// ignored — they don't affect identity and are reported when the
/// declaring module checks.
#[must_use]
pub fn resolve_registry_abilities(
    registry: &ModuleRegistry,
) -> Vec<(Arc<str>, Arc<crate::ability_resolver::DynAbility>)> {
    let mut infer = Infer::new();
    let mut discarded = Vec::new();
    let mut out = Vec::new();
    let mut modules: Vec<_> = registry
        .all_modules()
        .map(|info| (info.path.clone(), Arc::clone(&info.module)))
        .collect();
    modules.sort_by_key(|(path, _)| {
        (
            path.segments().first().map(AsRef::as_ref) != Some("platform"),
            path.to_string(),
        )
    });
    for (path, module) in modules {
        let namespace = path.to_string();
        for item in &module.items {
            if let crate::ast::ItemKind::Ability(def) = &item.kind {
                let dyn_ab = resolve_ability_def(&mut infer, def, &mut discarded);
                infer
                    .ability_resolver
                    .register_dynamic_in_namespace(&namespace, dyn_ab);
                if let Some(ability) = infer.ability_resolver.get_namespaced(&namespace, &def.name)
                {
                    out.push((
                        format!("{namespace}::{}", def.name).into(),
                        Arc::clone(ability),
                    ));
                }
            }
        }
    }
    out
}

/// Register the module's enum declarations and bring every visible
/// variant constructor into scope (prelude `Option`/`Result` plus locals;
/// locals shadow prelude variants of the same name).
fn register_enums(
    infer: &mut Infer,
    module: &crate::ast::Module,
    env: &mut TypeEnv,
    errors: &mut Vec<BoxedTypeError>,
) {
    for item in &module.items {
        if let crate::ast::ItemKind::Enum(enum_def) = &item.kind {
            // A declaration claiming a reserved prelude uuid must *be* the
            // canonical Option/Result declaration; anything else is an
            // attempted identity hijack (or a drifted core source).
            if let Err(message) = super::enums::validate_reserved_declaration(enum_def) {
                errors.push(Box::new(TypeError::new(
                    TypeErrorKind::InvalidDeclaration { message },
                    (enum_def.name_span.start, enum_def.name_span.end),
                )));
                continue;
            }
            infer.enum_registry.register_def(enum_def);
        }
    }

    // Synthetic binding ids for constructors, distinct from imports
    // (2_000_000+) and user bindings.
    let mut next_binding_id: BindingId = 4_000_000;
    let enums: Vec<_> = infer.enum_registry.iter().cloned().collect();
    for info in enums {
        for (idx, variant) in info.variants.iter().enumerate() {
            // Respect shadowing: only bind the variant if this enum is its
            // current owner.
            let owned = infer
                .enum_registry
                .resolve_variant(&variant.name)
                .is_some_and(|(owner, _)| owner.name == info.name);
            if !owned {
                continue;
            }
            let scheme = info.constructor_scheme(&mut infer.r#gen, idx);
            env.insert(next_binding_id, Arc::clone(&variant.name), scheme);
            next_binding_id += 1;
        }
    }
}

/// Substitute type parameters in a type with type variables.
pub(super) fn substitute_type_params(
    ty: &Type,
    type_var_map: &HashMap<Arc<str>, TypeVarId>,
) -> Type {
    match ty {
        Type::Named(named) => {
            // Check if this is a type parameter reference
            if named.args.is_empty()
                && let Some(&var_id) = type_var_map.get(&named.name)
            {
                return Type::var(var_id);
            }
            // Otherwise, recursively substitute in type arguments, preserving
            // any nominal identity (a declared enum's uuid).
            Type::Named(
                named.map_args(
                    named
                        .args
                        .iter()
                        .map(|arg| substitute_type_params(arg, type_var_map))
                        .collect(),
                ),
            )
        }
        Type::Function(f) => Type::function_with_abilities(
            f.params
                .iter()
                .map(|p| substitute_type_params(p, type_var_map))
                .collect(),
            substitute_type_params(&f.ret, type_var_map),
            f.abilities.clone(),
        ),
        Type::Tuple(elems) => Type::Tuple(
            elems
                .iter()
                .map(|e| substitute_type_params(e, type_var_map))
                .collect(),
        ),
        Type::Record(rec) => Type::Record(crate::types::RecordType::new(
            rec.fields
                .iter()
                .map(|(n, t)| (Arc::clone(n), substitute_type_params(t, type_var_map)))
                .collect(),
        )),
        // Primitives and other types pass through unchanged
        _ => ty.clone(),
    }
}

/// Check a module with cross-module imports resolved.
///
/// This is the main entry point for cross-module type checking. It resolves
/// imports from the registry and includes their types in the initial environment.
///
/// # Arguments
///
/// * `module` - The module to type check
/// * `module_path` - The path of this module in the package
/// * `registry` - The module registry containing all loaded modules
#[must_use]
pub fn check_module_with_registry(
    module: crate::ast::Module,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
) -> CheckResult {
    check_module_core(Infer::new(), module, Some((module_path, registry)))
}

/// Check a single module with a custom ability resolver.
///
/// Like [`check_module`], but the resolver decides which abilities are
/// in scope (e.g. an embedder-registered platform prelude).
#[must_use]
pub fn check_module_with_resolver(
    module: crate::ast::Module,
    resolver: AbilityResolver,
) -> CheckResult {
    check_module_core(Infer::with_resolver(resolver), module, None)
}

/// Check a module with cross-module support and a custom ability resolver.
///
/// This variant allows specifying which abilities are available at compile
/// time, which is useful for LSP and other tools that need to respect
/// package configuration.
#[must_use]
pub fn check_module_with_registry_and_resolver(
    module: crate::ast::Module,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    resolver: AbilityResolver,
) -> CheckResult {
    check_module_core(
        Infer::with_resolver(resolver),
        module,
        Some((module_path, registry)),
    )
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
            }
        }
    }

    bind_all_module_exports(infer, module_path, registry, &mut env, &mut next_binding_id);

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
                let qualified: Arc<str> = format!("{path}::{}", export.name).into();
                let binding_id = *next_binding_id;
                *next_binding_id += 1;
                env.insert(binding_id, qualified, scheme);
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
                // `with` clause on an export means pure.
                return Some(build_function_scheme(infer, func, false));
            }
            (crate::ast::ItemKind::Const(const_def), ExportKind::Const)
                if const_def.name.as_ref() == name =>
            {
                return Some(Scheme::mono(const_def.ty.clone()));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> crate::ast::Span {
        crate::ast::Span { start: 0, end: 0 }
    }

    fn method(
        name: &str,
        type_params: &[&str],
        params: &[(&str, Type)],
        ret_ty: Type,
    ) -> crate::ast::AbilityMethod {
        crate::ast::AbilityMethod {
            name: Arc::from(name),
            type_params: type_params
                .iter()
                .map(|name| crate::ast::TypeParam {
                    name: Arc::from(*name),
                    span: span(),
                })
                .collect(),
            params: params
                .iter()
                .map(|(name, ty)| (Arc::from(*name), ty.clone()))
                .collect(),
            ret_ty,
            span: span(),
        }
    }

    fn ability_module(name: &str, methods: Vec<crate::ast::AbilityMethod>) -> crate::ast::Module {
        crate::ast::Module {
            name: Arc::from("test"),
            doc: None,
            items: vec![crate::ast::Item {
                kind: crate::ast::ItemKind::Ability(crate::ast::AbilityDef {
                    name: Arc::from(name),
                    name_span: span(),
                    is_public: true,
                    dependencies: vec![],
                    methods,
                    resolved_id: None,
                }),
                span: span(),
                doc: None,
            }],
        }
    }

    /// A named type-parameter reference as the parser lowers it.
    fn ty_param(name: &str) -> Type {
        Type::Named(crate::types::NamedType::new(Arc::from(name), vec![]))
    }

    /// An in-language declaration must hash to the same identity as a
    /// descriptor-style rendering of the interface (method IDs are
    /// declaration indices, signatures render through the canonical type
    /// grammar): this is what lets host handlers keyed against the
    /// resolved declarations serve performs compiled from them.
    #[test]
    fn declaration_hashing_matches_descriptor_hashing() {
        use ambient_core::{MethodDescriptor, hash_interface};

        let mut module = ability_module(
            "Console",
            vec![
                method("print", &[], &[("message", Type::string())], Type::Unit),
                method("eprint", &[], &[("message", Type::string())], Type::Unit),
                method("println", &[], &[("message", Type::string())], Type::Unit),
            ],
        );

        let (abilities, errors) = resolve_ability_declarations(&mut module);
        assert!(errors.is_empty());
        assert_eq!(abilities.len(), 1);

        let expected = hash_interface(
            "Console",
            &[
                MethodDescriptor::new(0, "print", 1, |f| vec![f.string()], |f| f.unit()),
                MethodDescriptor::new(1, "eprint", 1, |f| vec![f.string()], |f| f.unit()),
                MethodDescriptor::new(2, "println", 1, |f| vec![f.string()], |f| f.unit()),
            ],
        );

        let console = &abilities[0];
        assert_eq!(console.id, expected);
        assert_eq!(console.method("print").map(|m| m.id), Some(0));
        assert_eq!(console.method("eprint").map(|m| m.id), Some(1));
        assert_eq!(console.method("println").map(|m| m.id), Some(2));

        // The identity is also written back for the compiler.
        let crate::ast::ItemKind::Ability(def) = &module.items[0].kind else {
            panic!("expected ability item");
        };
        assert_eq!(def.resolved_id, Some(console.id));
    }

    /// Generic methods are the risky parity case: the descriptor renders
    /// each `type_var()` occurrence as an independent `varN`, so the
    /// declaration must use a distinct type parameter per position
    /// (`run<T, R>` — never one parameter in two positions).
    #[test]
    fn generic_declaration_hashing_matches_descriptor_hashing() {
        use ambient_core::{MethodDescriptor, hash_interface};

        let list_of_string = Type::named("List", vec![Type::string()]);
        let mut module = ability_module(
            "Execute",
            vec![
                method(
                    "has_function",
                    &[],
                    &[("hash", Type::string())],
                    Type::bool(),
                ),
                method(
                    "get_dependencies",
                    &[],
                    &[("hash", Type::string())],
                    list_of_string.clone(),
                ),
                method(
                    "load_functions",
                    &[],
                    &[("bundle", Type::bytes())],
                    Type::Unit,
                ),
                method(
                    "run",
                    &["T", "R"],
                    &[("hash", Type::string()), ("args", ty_param("T"))],
                    ty_param("R"),
                ),
                method(
                    "get_functions",
                    &[],
                    &[("hashes", list_of_string)],
                    Type::bytes(),
                ),
                method(
                    "run_with",
                    &["T", "U", "R"],
                    &[
                        ("hash", Type::string()),
                        ("args", ty_param("T")),
                        ("handler", ty_param("U")),
                    ],
                    ty_param("R"),
                ),
            ],
        );

        let (abilities, errors) = resolve_ability_declarations(&mut module);
        assert!(errors.is_empty());

        let expected = hash_interface(
            "Execute",
            &[
                MethodDescriptor::new(0, "has_function", 1, |f| vec![f.string()], |f| f.bool()),
                MethodDescriptor::new(
                    1,
                    "get_dependencies",
                    1,
                    |f| vec![f.string()],
                    |f| f.list(f.string()),
                ),
                MethodDescriptor::new(2, "load_functions", 1, |f| vec![f.bytes()], |f| f.unit()),
                MethodDescriptor::new(
                    3,
                    "run",
                    2,
                    |f| vec![f.string(), f.type_var()],
                    |f| f.type_var(),
                ),
                MethodDescriptor::new(
                    4,
                    "get_functions",
                    1,
                    |f| vec![f.list(f.string())],
                    |f| f.bytes(),
                ),
                MethodDescriptor::new(
                    5,
                    "run_with",
                    3,
                    |f| vec![f.string(), f.type_var(), f.type_var()],
                    |f| f.type_var(),
                ),
            ],
        );

        let execute = &abilities[0];
        assert_eq!(execute.id, expected);
        assert_eq!(execute.method("run").map(|m| m.id), Some(3));
        assert_eq!(execute.method("run_with").map(|m| m.id), Some(5));
    }

    /// A module-level `const` must be in scope inside function bodies,
    /// regardless of declaration order. Regression test: consts used to be
    /// checked (their value against their annotation) but never registered
    /// into the module environment, so a reference from a function body
    /// resolved to `UndefinedVariable`.
    #[test]
    fn module_const_is_in_scope_in_function_bodies() {
        use crate::ast::{ConstDef, Expr, FunctionDef, Item, ItemKind, Module};

        // `fn use_it() = NANOS_PER_SEC + 1` is declared *before* the const it
        // references, to also cover forward references.
        let module = Module {
            name: Arc::from("test"),
            doc: None,
            items: vec![
                Item::new(
                    ItemKind::Function(FunctionDef {
                        name: Arc::from("use_it"),
                        name_span: span(),
                        is_public: false,
                        type_params: vec![],
                        params: vec![],
                        ret_ty: None,
                        abilities: vec![],
                        body: Expr::binary(
                            crate::ast::BinaryOp::Add,
                            Expr::name("NANOS_PER_SEC"),
                            Expr::number(1.0),
                        ),
                    }),
                    span(),
                ),
                Item::new(
                    ItemKind::Const(ConstDef {
                        name: Arc::from("NANOS_PER_SEC"),
                        name_span: span(),
                        is_public: false,
                        ty: Type::number(),
                        value: Expr::number(1_000_000_000.0),
                    }),
                    span(),
                ),
            ],
        };

        let result = check_module(module);
        assert!(
            result.errors.is_empty(),
            "const reference from a function body should type-check, got: {:?}",
            result.errors
        );
    }
}

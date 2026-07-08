//! Local declaration registration (Phase 1, second half) and declared-type
//! validation.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::BindingId;
use crate::fqn::{Fqn, ModuleId, NameKey};
use crate::types::{AbilityId, AbilitySet, TraitDef, TraitMethodDef, Type, TypeVarId};

use crate::infer::Infer;
use crate::infer::env::{Scheme, TypeEnv};
use crate::infer::error::{BoxedTypeError, BoxedTypeErrorExt, TypeError, TypeErrorKind};

use super::abilities::register_abilities;
use super::impls::register_inherent_impls;

/// Phase 1 registration of the current module's own declarations into the
/// checker: named types, unit-struct values, traits, enums, abilities,
/// function/const signatures, and inherent impls. `module_id` is the key
/// the module's own items bind under (`None` registry-less; see
/// [`bind_own_item`]).
pub(super) fn register_local_declarations(
    infer: &mut Infer,
    module: &mut crate::ast::Module,
    env: &mut TypeEnv,
    errors: &mut Vec<BoxedTypeError>,
    module_id: Option<&ModuleId>,
) {
    register_named_types(infer, module, module_id);
    // A struct claiming a reserved primitive identity must *be* the canonical
    // `extern` declaration (checked local-module only: foreign modules were
    // validated in their own check pass).
    validate_reserved_structs(module, errors);
    // Unit structs are values too: each denotes a single value constructed by
    // its bare name (like a nullary variant), bound so `let o = Origin` checks.
    register_unit_struct_values(module, env, module_id);
    register_traits(infer, module);
    register_enums(infer, module, env, errors, module_id);
    // ORDERING (load-bearing): `build_import_env` already registered
    // cross-module ability imports as bare dynamics; `register_abilities`
    // runs *after* and overwrites, so a local `ability` shadows an imported
    // one of the same bare name (matching the value/type shadowing rule).
    register_abilities(infer, module, errors);
    // Constants and functions register before any body is checked, so both
    // are referenceable from every function/const/impl body regardless of
    // declaration order.
    collect_function_signatures(infer, module, env, module_id);
    collect_const_signatures(infer, module, env, module_id);
    // Inherent method signatures register before any body is checked too, so
    // methods are callable from every function and impl body.
    register_inherent_impls(infer, module, errors);
    // Every local type is now registered, so a written annotation naming an
    // unknown type is unambiguously undefined: report it once, module-wide.
    // Runs last so self- and mutually-recursive type names already resolve.
    check_declared_types(infer, module, errors);
}
/// The `(name, type, is_public)` view shared by the two named-type items:
/// `struct` definitions and `type` aliases. Both register the same way — a name
/// resolving to a type in the inferencer's substitution table. For a non-`unique`
/// struct that type is a bare record, so it substitutes structurally exactly
/// like an alias; `unique` structs carry a `Type::Nominal` identity instead.
pub(super) fn named_type_def(item: &crate::ast::Item) -> Option<(&Arc<str>, &Type, bool)> {
    match &item.kind {
        crate::ast::ItemKind::Struct(s) => Some((&s.name, &s.ty, s.is_public)),
        crate::ast::ItemKind::TypeAlias(t) => Some((&t.name, &t.ty, t.is_public)),
        _ => None,
    }
}
/// Register all struct definitions and type aliases from a module into the
/// inferencer so their names resolve as types while checking.
///
/// Each local type registers under *both* keys: its bare name (the Type IR
/// / `resolve_holes` layer, and the bare-string `Type::method` dispatch
/// sites, resolve types by bare name) and — when the module has an identity
/// — its `Item(Fqn)`, the key a resolved same-module typed-record
/// constructor (`Money { … }`) looks up. The `Type::Nominal` uuid stays the
/// content identity; the `Fqn` is only the checker-side location key.
pub(super) fn register_named_types(
    infer: &mut Infer,
    module: &crate::ast::Module,
    module_id: Option<&ModuleId>,
) {
    for item in &module.items {
        if let Some((name, ty, _)) = named_type_def(item) {
            infer.register_type_alias(Arc::clone(name), ty.clone());
            if let Some(id) = module_id {
                infer.register_type_alias_item(
                    Fqn::new(id.clone(), vec![Arc::clone(name)]),
                    ty.clone(),
                );
            }
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
fn register_unit_struct_values(
    module: &crate::ast::Module,
    env: &mut TypeEnv,
    module_id: Option<&ModuleId>,
) {
    // Synthetic binding ids distinct from imports (2_000_000+) and enum
    // variant constructors (4_000_000+).
    let mut next_binding_id: BindingId = 5_000_000;
    for item in &module.items {
        if let crate::ast::ItemKind::Struct(s) = &item.kind
            && s.is_unit_value()
        {
            bind_own_item(
                env,
                module_id,
                next_binding_id,
                &s.name,
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
pub(super) fn register_trait_def(infer: &mut Infer, trait_def: &crate::ast::TraitDef) {
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
/// Validate every struct declaration against the reserved primitive specs.
///
/// The `Bool`/`Number`/`String`/`Binary` primitives are ordinary in-language
/// `extern` declarations in `core_lib`, but their identity is anchored by the
/// reserved uuids in `types.rs`. This pins the two together: a declaration that
/// claims a reserved uuid — or a reserved primitive *name* — must be the
/// canonical `extern` unit struct, so the core sources can never drift from the
/// anchors (they fail the build if they try) and no module can hijack a
/// primitive identity. Runs local-module only (the same drift the enum guard
/// catches for `Option`/`Result`).
fn validate_reserved_structs(module: &crate::ast::Module, errors: &mut Vec<BoxedTypeError>) {
    for item in &module.items {
        if let crate::ast::ItemKind::Struct(struct_def) = &item.kind
            && let Err(message) = crate::infer::enums::validate_reserved_struct(struct_def)
        {
            errors.push(Box::new(TypeError::new(
                TypeErrorKind::InvalidDeclaration { message },
                (struct_def.name_span.start, struct_def.name_span.end),
            )));
        }
    }
}
/// Register the module's enum declarations and bring every visible
/// variant constructor into scope (prelude `Option`/`Result` plus locals;
/// locals shadow prelude variants of the same name).
fn register_enums(
    infer: &mut Infer,
    module: &crate::ast::Module,
    env: &mut TypeEnv,
    errors: &mut Vec<BoxedTypeError>,
    module_id: Option<&ModuleId>,
) {
    for item in &module.items {
        if let crate::ast::ItemKind::Enum(enum_def) = &item.kind {
            // A declaration claiming a reserved prelude uuid must *be* the
            // canonical Option/Result declaration; anything else is an
            // attempted identity hijack (or a drifted core source).
            if let Err(message) = crate::infer::enums::validate_reserved_declaration(enum_def) {
                errors.push(Box::new(TypeError::new(
                    TypeErrorKind::InvalidDeclaration { message },
                    (enum_def.name_span.start, enum_def.name_span.end),
                )));
                continue;
            }
            infer
                .enum_registry
                .register_def(enum_def, module_id.cloned());
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
            // A variant reference can arrive under either key, so bind both:
            //   - bare `Variant` — a prelude variant (`None`/`Some`), or one
            //     reachable through an *enum* import (`use m::Enum`), which
            //     the resolve pass leaves bare;
            //   - `Fqn(<owning module>, [Enum, Variant])` — a same-module
            //     variant, or one imported *by variant* (`use m::{Variant}`),
            //     which the resolve pass canonicalizes.
            // The two keys never collide (bare vs. two-segment `Item`), and
            // the runtime tag stays the bare-named entry in the enum registry.
            env.insert(next_binding_id, Arc::clone(&variant.name), scheme.clone());
            next_binding_id += 1;
            if let Some(enum_module) = &info.module {
                let fqn = Fqn::new(
                    enum_module.clone(),
                    vec![Arc::clone(&info.name), Arc::clone(&variant.name)],
                );
                env.insert_item(next_binding_id, fqn, scheme);
                next_binding_id += 1;
            }
        }
    }
}
/// Bind one of the current module's own items into `env` under the same
/// key its same-module references resolve to: `Item(Fqn(module_id,
/// [name]))` when the module has an identity (registry check), else bare
/// (registry-less check, where the resolve pass never ran).
fn bind_own_item(
    env: &mut TypeEnv,
    module_id: Option<&ModuleId>,
    binding_id: BindingId,
    name: &Arc<str>,
    scheme: Scheme,
) {
    match module_id {
        Some(id) => env.insert_item(
            binding_id,
            Fqn::new(id.clone(), vec![Arc::clone(name)]),
            scheme,
        ),
        None => env.insert(binding_id, Arc::clone(name), scheme),
    }
}
/// Look up one of the current module's own items, mirroring
/// [`bind_own_item`]'s keying.
pub(super) fn own_item_scheme<'a>(
    env: &'a TypeEnv,
    module_id: Option<&ModuleId>,
    name: &str,
) -> Option<&'a Scheme> {
    match module_id {
        Some(id) => env.get_key(&NameKey::Item(Fqn::new(id.clone(), vec![Arc::from(name)]))),
        None => env.get_by_name(name),
    }
}
/// Collect function signatures into the environment.
fn collect_function_signatures(
    infer: &mut Infer,
    module: &crate::ast::Module,
    env: &mut TypeEnv,
    module_id: Option<&ModuleId>,
) {
    let mut next_binding_id: BindingId = 1_000_000;
    for item in &module.items {
        match &item.kind {
            crate::ast::ItemKind::Function(func) => {
                let binding_id = next_binding_id;
                next_binding_id += 1;
                let scheme = build_function_scheme(infer, func, true);
                bind_own_item(env, module_id, binding_id, &func.name, scheme);
            }
            crate::ast::ItemKind::ExternFn(def) => {
                let binding_id = next_binding_id;
                next_binding_id += 1;
                let scheme = build_extern_fn_scheme(infer, def);
                bind_own_item(env, module_id, binding_id, &def.name, scheme);
            }
            _ => {}
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
fn collect_const_signatures(
    infer: &mut Infer,
    module: &crate::ast::Module,
    env: &mut TypeEnv,
    module_id: Option<&ModuleId>,
) {
    let mut next_binding_id: BindingId = 2_000_000;
    for item in &module.items {
        if let crate::ast::ItemKind::Const(const_def) = &item.kind {
            let binding_id = next_binding_id;
            next_binding_id += 1;
            let ty = const_declared_type(infer, const_def);
            bind_own_item(
                env,
                module_id,
                binding_id,
                &const_def.name,
                Scheme::mono(ty),
            );
        }
    }
}
/// The type a `const` is registered under: its resolved annotation when
/// present, otherwise the type inferred from its literal initializer. A
/// non-literal initializer (already reported as `ConstNotLiteral`) falls back
/// to a fresh variable so downstream inference can proceed.
fn const_declared_type(infer: &mut Infer, const_def: &crate::ast::ConstDef) -> Type {
    match &const_def.ty {
        // `resolve_erroring` rewrites an undefined nominal to `Type::Error`
        // (reporting is done by the declared-types sweep), so the checked type
        // never carries an opaque `Named` — matching every other annotation.
        Some(annotation) => resolve_erroring(infer, annotation),
        None => crate::const_eval::literal_type(&const_def.value).unwrap_or_else(|| infer.fresh()),
    }
}
/// Build a type scheme for a function from its signature.
///
/// `infer_abilities` controls what an absent `with` clause means: for local
/// private functions (true) the scheme gets a fresh ability variable that
/// [`bind_inferred_abilities`] later binds to the body's inferred effects;
/// for public or foreign functions (false) it means "pure".
pub(super) fn build_function_scheme(
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

    // Build parameter types, resolving type aliases. An undefined type name
    // becomes `Type::Error` (reported once by the declared-types sweep) so the
    // scheme callers instantiate never carries an opaque `Named`.
    let param_types: Vec<Type> = func
        .params
        .iter()
        .map(|p| match &p.ty {
            Some(ty) => {
                let substituted = substitute_type_params(ty, &type_var_map);
                resolve_erroring(infer, &substituted)
            }
            None => infer.fresh(),
        })
        .collect();

    // Build return type, resolving type aliases
    let ret_ty = match &func.ret_ty {
        Some(ty) => {
            let substituted = substitute_type_params(ty, &type_var_map);
            resolve_erroring(infer, &substituted)
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
/// Build a type scheme for an `extern fn` from its declared signature.
///
/// Extern fns are pure by construction (the parser rejects a `with`
/// clause), so the ability set is always empty — never an inference
/// variable. The full signature is written (lowering enforces typed params
/// and a return type), so no holes remain beyond quantified type params.
pub(super) fn build_extern_fn_scheme(infer: &mut Infer, def: &crate::ast::ExternFnDef) -> Scheme {
    let mut type_var_map: HashMap<Arc<str>, TypeVarId> = HashMap::new();
    let mut quantified_vars = Vec::new();
    for tp in &def.type_params {
        let var_id = infer.r#gen.fresh_id();
        type_var_map.insert(Arc::clone(&tp.name), var_id);
        quantified_vars.push(var_id);
    }

    let param_types: Vec<Type> = def
        .params
        .iter()
        .map(|p| match &p.ty {
            Some(ty) => {
                let substituted = substitute_type_params(ty, &type_var_map);
                resolve_erroring(infer, &substituted)
            }
            // Unreachable through lowering (typed params are enforced), but
            // a hole is the graceful fallback for a hand-built AST.
            None => infer.fresh(),
        })
        .collect();

    let substituted_ret = substitute_type_params(&def.ret_ty, &type_var_map);
    let ret_ty = resolve_erroring(infer, &substituted_ret);

    let fn_ty = Type::function_with_abilities(param_types, ret_ty, AbilitySet::Empty);
    if quantified_vars.is_empty() {
        Scheme::mono(fn_ty)
    } else {
        Scheme::poly(quantified_vars, fn_ty)
    }
}

/// Substitute type parameters in a type with type variables.
pub(in crate::infer) fn substitute_type_params(
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
// ─────────────────────────────────────────────────────────────────────────────
// Resolve-or-error for type annotations
// ─────────────────────────────────────────────────────────────────────────────

/// Whether `name` denotes a type that exists in the module's world: a rigid
/// parameter in scope (`extra_known`), the `Self` placeholder, a built-in
/// structural container, a registered type alias/struct, or a registered enum.
/// The predicate a written type annotation must satisfy to not be "undefined".
///
/// The four primitives are **not** a special case here: they are ordinary
/// prelude imports registered as type aliases (`String`, `Number`, …), so a
/// registry-backed module finds them via `get_type_alias`. Deliberately *not*
/// falling back to `Primitive::from_name` keeps this predicate in lockstep with
/// [`Infer::expand_named_alias`], which no longer resolves primitives by name
/// without a registry. A stale primitive branch here would declare a bare
/// `String` "known" while resolution left it an opaque, uuid-less `Named` —
/// leaking past resolve-or-error into unification and signature hashes instead
/// of being sanitized to `Type::Error`. Registry-less checks (which never seed
/// the primitive aliases) therefore correctly report a bare `String`
/// annotation as `UndefinedTypeName`.
///
/// `Type::Param` (a resolved rigid parameter) never reaches the checks that
/// call this — it isn't a `Named` — so a body's own type parameters are never
/// flagged even without appearing in `extra_known`; the set only carries the
/// *unresolved* parameter names a raw-AST sweep sees.
fn is_known_type_name(
    infer: &Infer,
    name: &str,
    extra_known: &std::collections::HashSet<Arc<str>>,
) -> bool {
    extra_known.contains(name)
        || name == "Self"
        || matches!(name, "List" | "Map" | "Set")
        || infer.get_type_alias(name).is_some()
        || infer.enum_registry.get(name).is_some()
}
/// Report `UndefinedTypeName` for every unknown nominal head name in a
/// *written* (unresolved, raw-AST) type annotation, recursing into composite
/// children and type arguments.
///
/// Checks the **head** name, so a generic user type (`Pair<A, B>`) stays
/// valid while `Nope<A>` is flagged — an undefined head makes its arguments
/// moot. Reporting-only (does not rewrite): the declared-types sweep uses it
/// on raw AST types, which sidesteps `resolve_holes` (and its alias
/// expansion, which would loop on a self-referential struct).
fn report_undefined_types(
    infer: &Infer,
    ty: &Type,
    span: (u32, u32),
    extra_known: &std::collections::HashSet<Arc<str>>,
    errors: &mut Vec<BoxedTypeError>,
) {
    match ty {
        // `Handler<A>` / `Handler<A, R>`: the head is a builtin type
        // constructor (`resolve_holes` lowers it to `Type::Handler`), and its
        // first argument is an *ability* name resolved through the ability
        // namespace — not a type, so it must not be flagged here. Only the
        // optional answer type (`R`) is a real type annotation to check.
        Type::Named(n) if n.name.as_ref() == "Handler" && matches!(n.args.len(), 1 | 2) => {
            if let Some(answer) = n.args.get(1) {
                report_undefined_types(infer, answer, span, extra_known, errors);
            }
        }
        Type::Named(n) => {
            if !is_known_type_name(infer, &n.name, extra_known) {
                errors.push(Box::new(TypeError::new(
                    TypeErrorKind::UndefinedTypeName {
                        name: Arc::clone(&n.name),
                    },
                    span,
                )));
                return;
            }
            for arg in &n.args {
                report_undefined_types(infer, arg, span, extra_known, errors);
            }
        }
        Type::Tuple(elems) => {
            for e in elems {
                report_undefined_types(infer, e, span, extra_known, errors);
            }
        }
        Type::Record(rec) => {
            for (_, t) in &rec.fields {
                report_undefined_types(infer, t, span, extra_known, errors);
            }
        }
        Type::Function(f) => {
            for p in &f.params {
                report_undefined_types(infer, p, span, extra_known, errors);
            }
            report_undefined_types(infer, &f.ret, span, extra_known, errors);
        }
        Type::AbilityValue(av) => {
            report_undefined_types(infer, &av.result, span, extra_known, errors);
        }
        Type::Forall(fa) => report_undefined_types(infer, &fa.body, span, extra_known, errors),
        // Leaves, and already-resolved forms (`Nominal`, `Param`, `Var`,
        // primitives, `Unit`, ...): nothing to flag. `Nominal` inners belong
        // to some *other* declaration, already checked at its own site.
        _ => {}
    }
}
/// The rigid-parameter name set for a declaration's own type parameters.
fn type_param_set(params: &[crate::ast::TypeParam]) -> std::collections::HashSet<Arc<str>> {
    params.iter().map(|tp| Arc::clone(&tp.name)).collect()
}
/// Rewrite every unresolved nominal reference in a *resolved* type to
/// `Type::Error`, so an undefined type never leaks into unification or a
/// signature hash as an opaque `Named`. Non-reporting: the declared-types
/// sweep already reports these (see [`report_undefined_types`]); this just
/// keeps the checked/hashed type clean so no cascade or leak follows.
/// `Type::Error` unifies away, so downstream uses see no secondary error.
fn error_undefined_types(infer: &Infer, ty: &Type) -> Type {
    let empty = std::collections::HashSet::new();
    match ty {
        Type::Named(n) if n.uuid.is_none() && !is_known_type_name(infer, &n.name, &empty) => {
            Type::Error
        }
        Type::Named(n) => Type::Named(
            n.map_args(
                n.args
                    .iter()
                    .map(|a| error_undefined_types(infer, a))
                    .collect(),
            ),
        ),
        Type::Tuple(elems) => Type::Tuple(
            elems
                .iter()
                .map(|e| error_undefined_types(infer, e))
                .collect(),
        ),
        Type::Record(rec) => Type::Record(crate::types::RecordType::new(
            rec.fields
                .iter()
                .map(|(n, t)| (Arc::clone(n), error_undefined_types(infer, t)))
                .collect(),
        )),
        Type::Function(f) => Type::function_with_abilities(
            f.params
                .iter()
                .map(|p| error_undefined_types(infer, p))
                .collect(),
            error_undefined_types(infer, &f.ret),
            f.abilities.clone(),
        ),
        Type::AbilityValue(av) => {
            Type::ability_value(error_undefined_types(infer, &av.result), av.ability.clone())
        }
        Type::Forall(fa) => Type::Forall(crate::types::ForallType::with_abilities(
            fa.vars.clone(),
            fa.ability_vars.clone(),
            error_undefined_types(infer, &fa.body),
        )),
        // `Nominal` inner is another declaration's already-resolved body;
        // leave it (and every leaf) untouched.
        _ => ty.clone(),
    }
}
/// Resolve holes/aliases in a written annotation and rewrite any leftover
/// undefined nominal reference to `Type::Error`. The value-side counterpart
/// to reporting: every signature and body annotation runs through this so the
/// *checked* type never carries an opaque `Named`. Reporting is done once, by
/// the declared-types sweep, keeping diagnostics free of duplicates.
pub(super) fn resolve_erroring(infer: &mut Infer, ty: &Type) -> Type {
    let resolved = infer.resolve_holes(ty);
    error_undefined_types(infer, &resolved)
}
/// Resolve a *body-local* annotation — a `let` binding or a lambda parameter
/// type — reporting any undefined type name and rewriting it to
/// `Type::Error`. These annotations are the one kind the declared-types sweep
/// can't reach (they live inside expression bodies), so this both reports (to
/// `pending_errors`, drained module-wide) and rewrites, at their sole
/// resolution site in [`infer::expr`](crate::infer::expr). Rigid type parameters in
/// scope resolve to `Type::Param` first, so a body's own `T` is never flagged.
pub(in crate::infer) fn resolve_body_annotation(
    infer: &mut Infer,
    ty: &Type,
    span: (u32, u32),
) -> Type {
    let resolved = infer.resolve_holes(ty);
    let no_extra = std::collections::HashSet::new();
    let mut reported = Vec::new();
    report_undefined_types(infer, &resolved, span, &no_extra, &mut reported);
    infer.pending_errors.extend(reported);
    error_undefined_types(infer, &resolved)
}
/// Report undefined type names across every local declaration's written type
/// annotations (Phase 1, after all local types are registered so self- and
/// mutually-recursive names already resolve).
///
/// The single reporting authority for undefined types in declared
/// signatures: it walks raw AST types (foreign items untouched — only the
/// current module's `items`), so the scheme builders and body checkers can
/// rewrite to `Type::Error` without also reporting, keeping each undefined
/// type exactly one diagnostic. In-body `let`/lambda annotations are the one
/// exception — reported inline in `infer::expr`, as they never reach here.
fn check_declared_types(
    infer: &Infer,
    module: &crate::ast::Module,
    errors: &mut Vec<BoxedTypeError>,
) {
    let empty = std::collections::HashSet::new();
    for item in &module.items {
        let span = (item.span.start, item.span.end);
        match &item.kind {
            crate::ast::ItemKind::Function(func) => {
                let known = type_param_set(&func.type_params);
                for p in &func.params {
                    if let Some(ty) = &p.ty {
                        let s = (p.span.start, p.span.end);
                        report_undefined_types(infer, ty, s, &known, errors);
                    }
                }
                if let Some(ret) = &func.ret_ty {
                    report_undefined_types(infer, ret, span, &known, errors);
                }
            }
            crate::ast::ItemKind::ExternFn(def) => {
                let known = type_param_set(&def.type_params);
                for p in &def.params {
                    if let Some(ty) = &p.ty {
                        let s = (p.span.start, p.span.end);
                        report_undefined_types(infer, ty, s, &known, errors);
                    }
                }
                report_undefined_types(infer, &def.ret_ty, span, &known, errors);
            }
            crate::ast::ItemKind::Const(c) => {
                if let Some(ty) = &c.ty {
                    report_undefined_types(infer, ty, span, &empty, errors);
                }
            }
            crate::ast::ItemKind::Struct(s) => {
                let known = type_param_set(&s.type_params);
                let s_span = (s.name_span.start, s.name_span.end);
                for field_ty in struct_field_types(&s.ty) {
                    report_undefined_types(infer, field_ty, s_span, &known, errors);
                }
            }
            crate::ast::ItemKind::Enum(e) => {
                let known = type_param_set(&e.type_params);
                for v in &e.variants {
                    if let Some(payload) = &v.payload {
                        let s = (v.span.start, v.span.end);
                        report_undefined_types(infer, payload, s, &known, errors);
                    }
                }
            }
            crate::ast::ItemKind::Trait(t) => {
                for m in &t.methods {
                    let known = type_param_set(&m.type_params);
                    let s = (m.span.start, m.span.end);
                    for (_, pty) in &m.params {
                        report_undefined_types(infer, pty, s, &known, errors);
                    }
                    report_undefined_types(infer, &m.ret_ty, s, &known, errors);
                }
            }
            crate::ast::ItemKind::Ability(a) => {
                for m in &a.methods {
                    let known = type_param_set(&m.type_params);
                    let s = (m.span.start, m.span.end);
                    for (_, pty) in &m.params {
                        report_undefined_types(infer, pty, s, &known, errors);
                    }
                    report_undefined_types(infer, &m.ret_ty, s, &known, errors);
                }
            }
            crate::ast::ItemKind::Impl(imp) => {
                // The impl target (`impl Strng`) is validated elsewhere
                // (invalid-target / structural-type errors); only the method
                // signatures are swept here.
                let impl_known = type_param_set(&imp.type_params);
                for m in &imp.methods {
                    let method_known: std::collections::HashSet<Arc<str>> = impl_known
                        .iter()
                        .cloned()
                        .chain(m.type_params.iter().map(|tp| Arc::clone(&tp.name)))
                        .collect();
                    for p in &m.params {
                        if let Some(ty) = &p.ty {
                            let s = (p.span.start, p.span.end);
                            report_undefined_types(infer, ty, s, &method_known, errors);
                        }
                    }
                    if let Some(ret) = &m.ret_ty {
                        let s = (m.span.start, m.span.end);
                        report_undefined_types(infer, ret, s, &method_known, errors);
                    }
                }
            }
            _ => {}
        }
    }
}
/// The field types of a struct declaration's stored type. A declared struct
/// is a `Type::Nominal` wrapping a `Record` (a non-`unique` struct is the
/// bare `Record`); either way its fields are the written annotations.
fn struct_field_types(ty: &Type) -> Vec<&Type> {
    let record = match ty {
        Type::Nominal(n) => &*n.inner,
        other => other,
    };
    match record {
        Type::Record(rec) => rec.fields.iter().map(|(_, t)| t).collect(),
        _ => Vec::new(),
    }
}

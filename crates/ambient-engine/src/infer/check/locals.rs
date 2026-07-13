//! Local declaration registration (Phase 1, second half) and declared-type
//! validation.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::BindingId;
use crate::fqn::{Fqn, ModuleId, NameKey};
use crate::types::{AbilitySet, TraitDef, TraitMethodDef, Type, TypeVarId};

use crate::infer::Infer;
use crate::infer::env::{AliasTarget, Scheme, TypeEnv};
use crate::infer::error::{BoxedTypeError, TypeError, TypeErrorKind};

use super::abilities::register_abilities;
use super::declared_types::{check_declared_types, resolve_erroring};
use super::impls::register_inherent_impls;
use super::subst::substitute_type_params;

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
    register_traits(infer, module, errors, module_id);
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
/// The `(name, target, is_public)` view shared by the two named-type items:
/// `struct` definitions and `type` aliases. Both register the same way — a
/// name resolving to an [`AliasTarget`] in the inferencer's alias table. For
/// a non-`unique` struct that target is its bare record, substituting
/// structurally exactly like an alias; `unique` structs carry a
/// `Type::Nominal` identity; a generic `extern` unit struct registers as an
/// opaque head (see [`AliasTarget::of_struct`]).
pub(super) fn named_type_def(item: &crate::ast::Item) -> Option<(&Arc<str>, AliasTarget, bool)> {
    match &item.kind {
        crate::ast::ItemKind::Struct(s) => Some((&s.name, AliasTarget::of_struct(s), s.is_public)),
        crate::ast::ItemKind::TypeAlias(t) => {
            Some((&t.name, AliasTarget::Whole(t.ty.clone()), t.is_public))
        }
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
        if let Some((name, target, _)) = named_type_def(item) {
            infer.register_type_alias_target(Arc::clone(name), target.clone());
            if let Some(id) = module_id {
                infer
                    .register_type_alias_item(Fqn::new(id.clone(), vec![Arc::clone(name)]), target);
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
fn register_traits(
    infer: &mut Infer,
    module: &crate::ast::Module,
    errors: &mut Vec<BoxedTypeError>,
    module_id: Option<&ModuleId>,
) {
    for item in &module.items {
        if let crate::ast::ItemKind::Trait(trait_def) = &item.kind {
            // Trait-level type parameters (generic traits) and supertraits both
            // parse but are not implemented — silent acceptance would drop them
            // and miscompile, so reject loudly at the declaration site. (Method
            // bounds — `fn m<U: Eq>` — *are* supported and live on the methods,
            // not here.)
            if let Err(message) = validate_supported_trait_shape(trait_def) {
                errors.push(Box::new(TypeError::new(
                    TypeErrorKind::InvalidDeclaration { message },
                    (trait_def.name_span.start, trait_def.name_span.end),
                )));
                continue;
            }
            // A declaration claiming a reserved trait uuid must *be* the
            // canonical prelude trait — the same hijack guard reserved
            // enums and primitives get.
            if let Err(message) = validate_reserved_trait(trait_def) {
                errors.push(Box::new(TypeError::new(
                    TypeErrorKind::InvalidDeclaration { message },
                    (trait_def.name_span.start, trait_def.name_span.end),
                )));
                continue;
            }
            register_trait_def(infer, trait_def, module_id);
        }
    }
}

/// Reject the two trait-declaration features that parse but are not yet
/// implemented: trait-level type parameters (generic traits like
/// `trait Container<T>`) and supertraits (`trait Sub with Base`). Both would
/// otherwise be silently dropped by `checked_trait_def` and miscompile.
fn validate_supported_trait_shape(def: &crate::ast::TraitDef) -> Result<(), String> {
    if !def.type_params.is_empty() {
        return Err(format!(
            "generic traits are not supported yet: trait `{}` declares trait-level \
             type parameters. Method-level type parameters (`fn method<T: Bound>(...)`) \
             are supported; move the parameter onto the method if that fits.",
            def.name
        ));
    }
    if !def.supertraits.is_empty() {
        return Err(format!(
            "supertraits are not supported yet: trait `{}` declares a `with` \
             supertrait clause. Declare the required methods on the trait directly, \
             or bound the implementing code on both traits.",
            def.name
        ));
    }
    Ok(())
}

/// Validate a trait declaration against the reserved core trait identities.
///
/// The operator traits (and `Default`) are ordinary declarations in
/// `core::traits`, but the engine's operator desugar anchors on their
/// reserved uuids ([`crate::types::ReservedTrait`]). A declaration claiming
/// one of those uuids must carry the canonical name and method shape, so the
/// core sources cannot drift from the anchors and no other module can hijack
/// an operator's dispatch identity.
fn validate_reserved_trait(def: &crate::ast::TraitDef) -> Result<(), String> {
    let Some(reserved) = crate::types::ReservedTrait::from_uuid(def.uuid) else {
        return Ok(());
    };

    let mismatch = |what: &str| {
        Err(format!(
            "`unique({})` is the reserved identity of the core trait `{}`; \
             a declaration using it must match the canonical shape exactly ({what})",
            crate::types::uuid_to_source(&def.uuid),
            reserved.name(),
        ))
    };

    if def.name.as_ref() != reserved.name() {
        return mismatch(&format!(
            "expected name `{}`, found `{}`",
            reserved.name(),
            def.name
        ));
    }
    let (method_name, has_self, param_count) = reserved_trait_method_shape(reserved);
    let [method] = def.methods.as_slice() else {
        return mismatch(&format!("expected exactly one method `{method_name}`"));
    };
    if method.name.as_ref() != method_name
        || method.has_self != has_self
        || method.params.len() != param_count
    {
        return mismatch(&format!(
            "expected method `{method_name}` with {param_count} parameter(s){}",
            if has_self { " and `self`" } else { "" }
        ));
    }
    // Operator desugaring anchors on the reserved shape and cannot supply
    // method-level generics (a fresh type or effect-row variable per operator
    // use), so a reserved operator method must be non-generic.
    if !method.type_params.is_empty() {
        return mismatch("its method must not declare method-level generics");
    }
    Ok(())
}

/// The canonical method shape of a reserved core trait:
/// `(method name, takes self, non-self parameter count)`.
fn reserved_trait_method_shape(
    reserved: crate::types::ReservedTrait,
) -> (&'static str, bool, usize) {
    use crate::types::ReservedTrait as R;
    match reserved {
        R::Add => ("add", true, 1),
        R::Sub => ("sub", true, 1),
        R::Mul => ("mul", true, 1),
        R::Div => ("div", true, 1),
        R::Mod => ("rem", true, 1),
        R::Eq => ("eq", true, 1),
        R::Ord => ("cmp", true, 1),
        R::Default => ("default", false, 0),
        R::Show => ("show", true, 0),
    }
}
/// Register a single trait definition into the trait registry, binding its
/// bare name in scope. The identity is the declaration's `unique(<uuid>)`
/// prefix, so re-registering the same trait (an import seen from several
/// modules) is idempotent.
pub(super) fn register_trait_def(
    infer: &mut Infer,
    trait_def: &crate::ast::TraitDef,
    module_id: Option<&ModuleId>,
) {
    infer
        .trait_registry
        .register_trait(checked_trait_def(trait_def, module_id));
}

/// Register a trait definition by identity only — resolvable by uuid for
/// impls and bounds, but its bare name does not enter scope. Used for
/// foreign traits this module never imported.
pub(super) fn register_trait_def_unnamed(
    infer: &mut Infer,
    trait_def: &crate::ast::TraitDef,
    module_id: Option<&ModuleId>,
) {
    infer
        .trait_registry
        .register_trait_unnamed(checked_trait_def(trait_def, module_id));
}

/// Convert an AST trait declaration to its checker form, stamping the
/// trait's `Fqn` (its build-global lookup key) from its defining module.
fn checked_trait_def(trait_def: &crate::ast::TraitDef, module_id: Option<&ModuleId>) -> TraitDef {
    let methods: Vec<TraitMethodDef> = trait_def
        .methods
        .iter()
        .map(|m| {
            // Method-level generics: type parameters (`U`, bounded or not) and
            // ability (row) variables (`E!`). Each type parameter's bounds are
            // captured separately in `dict_params` order (the single authority
            // the compiled impl method allocates its trailing dictionaries
            // from); the signature itself stays un-instantiated (see
            // `TraitMethodDef`).
            let mut type_param_names = Vec::new();
            let mut ability_var_names = Vec::new();
            for tp in &m.type_params {
                if tp.is_ability {
                    ability_var_names.push(Arc::clone(&tp.name));
                } else {
                    type_param_names.push(Arc::clone(&tp.name));
                }
            }
            let method_bounds: Vec<(Arc<str>, crate::ast::QualifiedName)> =
                crate::ast::dict_params(&m.type_params)
                    .into_iter()
                    .map(|(param, bound)| (param, bound.clone()))
                    .collect();
            TraitMethodDef::new(
                Arc::clone(&m.name),
                m.has_self,
                m.params.iter().map(|(_, ty)| ty.clone()).collect(),
                m.ret_ty.clone(),
            )
            .with_generics(
                m.abilities.clone(),
                type_param_names,
                ability_var_names,
                method_bounds,
            )
        })
        .collect();

    let fqn = module_id.map(|id| Fqn::new(id.clone(), vec![Arc::clone(&trait_def.name)]));
    TraitDef {
        uuid: trait_def.uuid,
        name: Arc::clone(&trait_def.name),
        fqn,
        methods,
    }
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
        let crate::ast::ItemKind::Struct(struct_def) = &item.kind else {
            continue;
        };
        let result = crate::infer::enums::validate_reserved_struct(struct_def)
            .and_then(|()| crate::infer::enums::validate_reserved_container(struct_def));
        if let Err(message) = result {
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
    // Synthetic binding ids in the consts' own range. The channel bases
    // are 1_000_000 own functions, 2_000_000 foreign exports, 3_000_000
    // consts, 4_000_000 enum constructors, 5_000_000 unit structs — and
    // they must stay disjoint: a shared id makes two names alias one
    // `bindings` slot, and the last write silently replaces the other
    // name's scheme. Consts briefly shared the foreign base, and because
    // foreign ids are assigned in map-iteration order, *which* import
    // lost its scheme to a const varied run to run — package builds with
    // a `const` failed nondeterministically, ~1 compile in 10.
    let mut next_binding_id: BindingId = 3_000_000;
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
    // Split the generics into type variables and ability (row) variables
    // (`E!`), allocating fresh quantified ids for each.
    let scope = super::ability_vars::generic_scope(infer, &func.type_params);

    // Resolve the signature with the item's ability variables in scope, so
    // `with E` positions bind to the row variable. `report_type_misuse` is
    // false here: an `E`-used-as-a-type mistake reports once, from the body
    // check (this same resolution runs twice, scheme and body).
    let (param_types, ret_ty, abilities) =
        infer.with_ability_var_scope(scope.ability_var_map.clone(), false, |infer| {
            // Parameter and return types, resolving type aliases. An
            // undefined name becomes `Type::Error` (reported once by the
            // declared-types sweep) so a caller never instantiates an opaque
            // `Named`.
            let param_types: Vec<Type> = func
                .params
                .iter()
                .map(|p| match &p.ty {
                    Some(ty) => {
                        let substituted = substitute_type_params(ty, &scope.type_var_map);
                        resolve_erroring(infer, &substituted)
                    }
                    None => infer.fresh(),
                })
                .collect();
            let ret_ty = match &func.ret_ty {
                Some(ty) => {
                    let substituted = substitute_type_params(ty, &scope.type_var_map);
                    resolve_erroring(infer, &substituted)
                }
                None => infer.fresh(),
            };

            // The declared `with` clause. Bare names that name an ability
            // variable form the row tail; other names resolve concrete
            // (unknown ones report). An absent clause means "inferred" for a
            // private function, "pure" for a public or foreign one.
            let abilities = if func.abilities.is_empty() {
                if infer_abilities && !func.is_public {
                    infer.fresh_ability_var()
                } else {
                    AbilitySet::Empty
                }
            } else {
                super::ability_vars::resolve_declared_with(
                    infer,
                    &func.abilities,
                    &scope.ability_var_map,
                    &func.name,
                )
            };
            (param_types, ret_ty, abilities)
        });

    let fn_ty = Type::function_with_abilities(param_types, ret_ty, abilities);

    let scheme = if scope.is_empty() {
        Scheme::mono(fn_ty)
    } else {
        Scheme::poly_with_abilities(
            scope.quantified_type_vars.clone(),
            scope.ability_vars.clone(),
            fn_ty,
        )
    };
    attach_scheme_bounds(infer, scheme, &func.type_params, &scope.type_var_map)
}

/// Attach an item's declared trait bounds to its scheme, resolving each
/// bound reference to a trait identity. The order comes from
/// [`crate::ast::dict_params`] — the same authority the compiler allocates
/// hidden dictionary parameters from. Resolution goes through
/// [`Infer::trait_uuid_of`], which prefers the resolve pass's canonical
/// `Fqn`: a foreign scheme's bound resolves in its defining module, never
/// re-resolved in this consumer's scope. Unknown bounds report through
/// `pending_errors`.
pub(super) fn attach_scheme_bounds(
    infer: &mut Infer,
    scheme: Scheme,
    type_params: &[crate::ast::TypeParam],
    type_var_map: &HashMap<Arc<str>, TypeVarId>,
) -> Scheme {
    let mut bounds = Vec::new();
    for (param, bound) in crate::ast::dict_params(type_params) {
        let Some(&var) = type_var_map.get(&param) else {
            continue;
        };
        let Some(trait_uuid) = infer.trait_uuid_of(bound) else {
            let span = type_params
                .iter()
                .find(|tp| tp.name == param)
                .map_or((0, 0), |tp| (tp.span.start, tp.span.end));
            infer.pending_errors.push(Box::new(TypeError::new(
                TypeErrorKind::UnknownTrait {
                    name: Arc::clone(&bound.name),
                },
                span,
            )));
            continue;
        };
        bounds.push((
            var,
            crate::types::TraitBound {
                trait_uuid,
                name: Arc::clone(&bound.name),
            },
        ));
    }
    if bounds.is_empty() {
        scheme
    } else {
        scheme.with_bounds(bounds)
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

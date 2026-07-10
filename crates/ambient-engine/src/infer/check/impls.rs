//! Impl-block checking (Phase 2): trait impls, inherent impls, and their
//! method schemes and dispatch symbols.

use std::collections::HashMap;
use std::sync::Arc;

use crate::types::{AbilitySet, TraitDef, Type, TypeVarId};

use crate::infer::env::{Scheme, TypeEnv};
use crate::infer::error::{BoxedTypeError, BoxedTypeErrorExt, TypeError, TypeErrorKind};
use crate::infer::expr::substitute_self;
use crate::infer::{Infer, inherent};

use super::bodies::DeferredAbilityCheck;
use super::locals::resolve_erroring;
use super::locals::substitute_type_params;

/// Check impl blocks and register implementations.
pub(super) fn check_impls(
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
    let Some(trait_uuid) = infer.trait_registry.lookup_trait(&trait_name.name) else {
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
    let Some(trait_def) = infer.trait_registry.get_trait(trait_uuid).cloned() else {
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
    let mut impl_record = crate::types::TraitImpl::new(trait_uuid, nominal_type.clone());
    for method in &mut impl_def.methods {
        let symbol =
            crate::types::impl_method_symbol(&nominal_type.uuid, &trait_uuid, &method.name);
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
                |ty| resolve_erroring(infer, ty),
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
// ─────────────────────────────────────────────────────────────────────────────
// Inherent impls
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve an inherent impl's target type to its coherence key.
///
/// Returns `None` when the target cannot carry inherent methods: a
/// structural type (record, tuple, function) or a bare impl type parameter
/// (which would be a blanket impl).
pub(super) fn inherent_impl_target(
    infer: &mut Infer,
    impl_def: &crate::ast::ImplDef,
) -> Option<(inherent::ImplKey, Type)> {
    let for_type = infer.resolve_holes(&impl_def.for_type);

    // `impl<T> T` — a blanket impl over every type — is not a thing. The
    // target resolves at registration (no rigid scope), so a bare parameter
    // is a `Named`; a `Param` is matched too in case a rigid scope is ever
    // live here.
    let blanket = match &for_type {
        Type::Named(n) if n.args.is_empty() => Some(n.name.as_ref()),
        Type::Param(name) => Some(name.as_ref()),
        _ => None,
    };
    if let Some(name) = blanket
        && impl_def
            .type_params
            .iter()
            .any(|tp| tp.name.as_ref() == name)
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
pub(super) fn register_inherent_impls(
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
        // Beyond being keyable, a `Named` target must actually exist. After
        // `resolve_holes`, every real nominal `Named` carries a uuid — a
        // declared enum, a prelude enum (`Option`/`Result`), or a built-in
        // container (`List`/`Map`/`Set`) — so a `uuid`-less `Named` is an
        // undefined head. Nominal targets (declared structs, `extern` types,
        // the primitives) are always real, taking the `_ => true` branch.
        let target = target.filter(|(_, for_type)| match for_type {
            Type::Named(n) => n.uuid.is_some() || infer.enum_registry.get(&n.name).is_some(),
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
pub(super) fn build_inherent_method_scheme(
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
    resolve_erroring(infer, &ty)
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
/// Type parameters stay rigid (`Type::Param`) inside bodies, like generic
/// function bodies — the impl's own parameters plus each method's. The impl
/// target is resolved under the same rigid scope, so `self` and the method's
/// annotations agree on `Param` (not one `Named`, one `Param`). Each body's
/// inferred abilities are recorded for deferred enforcement against the
/// method's `with` clause (no clause means pure, like a public function).
fn check_inherent_impl_bodies(
    infer: &mut Infer,
    impl_def: &mut crate::ast::ImplDef,
    env: &TypeEnv,
    errors: &mut Vec<BoxedTypeError>,
    deferred: &mut Vec<DeferredAbilityCheck>,
) {
    // Cloned so the per-method closure can resolve the target without holding
    // an immutable borrow of `impl_def` while `method` borrows it mutably.
    let for_type_ast = impl_def.for_type.clone();
    let impl_params: Vec<Arc<str>> = impl_def
        .type_params
        .iter()
        .map(|tp| Arc::clone(&tp.name))
        .collect();

    for method in &mut impl_def.methods {
        if method.resolved_symbol.is_none() {
            // Registration rejected the whole impl (invalid target); the
            // error is already reported.
            continue;
        }

        infer.reset_abilities();

        let rigid: Vec<Arc<str>> = impl_params
            .iter()
            .cloned()
            .chain(method.type_params.iter().map(|tp| Arc::clone(&tp.name)))
            .collect();
        infer.with_rigid_params(rigid, |infer| {
            let for_type = infer.resolve_holes(&for_type_ast);
            let mut func_env = env.extend();

            if method.has_self {
                func_env.insert_mono(method.self_id, Arc::from("self"), for_type.clone());
            }
            for param in &method.params {
                let param_ty = match &param.ty {
                    Some(ty) => {
                        let ty = substitute_self(ty, &for_type);
                        resolve_erroring(infer, &ty)
                    }
                    None => infer.fresh(),
                };
                func_env.insert_mono(param.id, Arc::clone(&param.name), param_ty);
            }

            let expected_ret = method.ret_ty.as_ref().map(|ty| {
                let ty = substitute_self(ty, &for_type);
                resolve_erroring(infer, &ty)
            });

            match infer.infer_expr(&func_env, &mut method.body) {
                Ok(body_ty) => {
                    if let Some(expected) = &expected_ret {
                        let method_span = (method.span.start, method.span.end);
                        if let Err(e) = infer.unify(expected, &body_ty, method_span) {
                            errors.push(
                                e.with_context(format!("in inherent method `{}`", method.name)),
                            );
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
        });
    }
}

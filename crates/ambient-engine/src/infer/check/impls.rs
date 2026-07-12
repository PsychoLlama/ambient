//! Impl-block checking (Phase 2): trait impls, inherent impls, and their
//! method schemes and dispatch symbols.

use std::collections::HashMap;
use std::sync::Arc;

use crate::types::{TraitDef, Type, TypeVarId};

use crate::infer::env::{Scheme, TypeEnv};
use crate::infer::error::{BoxedTypeError, BoxedTypeErrorExt, TypeError, TypeErrorKind};
use crate::infer::expr::substitute_self;
use crate::infer::{Infer, inherent};

use super::bodies::DeferredAbilityCheck;
use super::declared_types::resolve_erroring;
use super::subst::substitute_type_params;

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

    let for_type = infer.resolve_holes(&impl_def.for_type);

    // Generic (conditional) trait impls need machinery this path lacks:
    // an impl block that declares its own type parameters
    // (`impl<T: Eq> Eq for Pair<T>`), or a target applied to type arguments
    // (`impl Eq for List<T>`, and every container — they're all generic).
    // Coherence keys on the target's UUID alone, which cannot tell
    // `Option<Number>` from `Option<String>`, so registering one would
    // misdispatch. Reject loudly; a non-generic target (a plain enum or
    // struct) is fine. Containers await the conditional-impls task.
    let has_type_args = matches!(&for_type, Type::Named(n) if !n.args.is_empty());
    if !impl_def.type_params.is_empty() || has_type_args {
        errors.push(Box::new(TypeError::new(
            TypeErrorKind::ConditionalImplUnsupported {
                trait_name: Arc::clone(&trait_name.name),
                ty: for_type.clone(),
            },
            span,
        )));
        return;
    }

    // The target must carry a nominal identity — the same "what can be an
    // impl target" question the inherent path answers. A declared struct,
    // an `extern`/primitive nominal, and a declared/prelude enum all do;
    // a structural type or an unknown head does not.
    let Some((type_uuid, type_name)) = inherent::trait_impl_identity(&for_type) else {
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
    let mut impl_record = crate::types::TraitImpl::new(trait_uuid, type_uuid, type_name);
    for method in &mut impl_def.methods {
        let symbol = crate::types::impl_method_symbol(&type_uuid, &trait_uuid, &method.name);
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
/// An impl method inherits the *whole* trait method signature — parameter and
/// return types, and the declared effect row (`with E`). The impl author does
/// not (and cannot) re-declare that row: the method's effects are fixed by the
/// trait contract, so an explicit `with` clause on an impl method is still an
/// error. But the body is no longer forced pure. Instead it is checked under
/// the trait method's own generics — method-level type parameters rigid, `E!`
/// installed as a fresh row scope — exactly like an inherent method's body.
/// A body that only performs the polymorphic row (by calling a row-typed
/// parameter) type-checks; a body performing a *concrete* ability outside the
/// trait's declared row is rejected by the deferred subset check against the
/// trait method's declared abilities. Without this, dot dispatch and operator
/// overloading could launder arbitrary effects past callers, and effect
/// polymorphism could never reach a trait-dispatched method.
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
        // The method's effects are fixed by the trait signature; a `with`
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

        check_impl_method_body(infer, method, tm, for_type, env, errors, deferred);
    }
}

/// Check one trait-impl method body against the (Self-substituted) trait
/// signature, under the impl method's own method-level generics: type
/// parameters rigid, ability (row) variables installed as a fresh scope.
///
/// Because an impl method's parameters must be annotated (the grammar has no
/// bare `self, f` form), the impl re-declares the trait method's generics —
/// `fn each<E!>(self, f: (Number) -> () with E)` — and those annotations
/// resolve against these scopes, exactly like an inherent method. The trait
/// method's declared row still governs *enforcement*: `tm.abilities` is the
/// deferred check's declared set.
fn check_impl_method_body(
    infer: &mut Infer,
    method: &mut crate::ast::ImplMethod,
    tm: &crate::types::TraitMethodDef,
    for_type: &Type,
    env: &TypeEnv,
    errors: &mut Vec<BoxedTypeError>,
    deferred: &mut Vec<DeferredAbilityCheck>,
) {
    infer.reset_abilities();

    // The impl method's own generics scope the body: type parameters (`U`)
    // stay rigid (`Type::Param`), and each `E!` gets a fresh row variable, so
    // a `with E` position in an annotated parameter type resolves to that row.
    let rigid: Vec<Arc<str>> = method
        .type_params
        .iter()
        .filter(|tp| !tp.is_ability)
        .map(|tp| Arc::clone(&tp.name))
        .collect();
    let ability_scope = super::ability_vars::ability_var_scope(infer, &method.type_params);

    infer.with_ability_var_scope(ability_scope, true, |infer| {
        infer.with_rigid_params(rigid, |infer| {
            let mut func_env = env.extend();

            if tm.has_self {
                func_env.insert_mono(method.self_id, Arc::from("self"), for_type.clone());
            }

            // Each parameter's expected type comes from the trait signature
            // (Self → the implementing type, resolved under the scopes above);
            // an explicit annotation on the impl method is resolved the same
            // way and used in its place.
            for (param, expected_ty) in method.params.iter().zip(tm.params.iter()) {
                let param_ty = match &param.ty {
                    Some(ty) => resolve_erroring(infer, ty),
                    None => resolve_erroring(infer, &substitute_self(expected_ty, for_type)),
                };
                func_env.insert_mono(param.id, Arc::clone(&param.name), param_ty);
            }

            let expected_ret = resolve_erroring(infer, &substitute_self(&tm.ret, for_type));
            match infer.infer_expr(&func_env, &mut method.body) {
                Ok(body_ty) => {
                    let method_span = (method.span.start, method.span.end);
                    if let Err(e) = infer.unify(&expected_ret, &body_ty, method_span) {
                        errors.push(e.with_context(format!("in impl method `{}`", method.name)));
                    }
                    // The declared row is the trait method's `with` clause: a
                    // row-variable name resolves to nothing concrete and is
                    // ignored (its instantiation is enforced at call sites),
                    // so only a concrete ability outside the row is flagged.
                    deferred.push(DeferredAbilityCheck {
                        context: format!("trait impl method `{}`", method.name),
                        declared: tm.abilities.clone(),
                        inferred: infer.current_abilities().clone(),
                        span: method.span,
                    });
                }
                Err(e) => {
                    errors.push(e.with_context(format!("in impl method `{}`", method.name)));
                }
            }

            // A trait impl method body may call bounded generics at concrete
            // types; solve those constraints against this body before the next
            // one runs.
            infer.finish_body_constraints(&mut method.body, errors);
        });
    });
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
            let scheme = build_inherent_method_scheme(
                infer,
                &impl_type_params,
                method,
                &for_type,
                errors,
                false,
            );
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
    lenient_bounds: bool,
) -> Scheme {
    // Combined impl-then-method generics: the impl block's parameters (their
    // bounds scope every method) come first, then the method's own — the same
    // order the compiler allocates dictionary parameters in. Split into type
    // variables and ability (row) variables (`E!`), allocating fresh
    // quantified ids for each — mirroring `build_function_scheme`. The
    // hand-rolled positional minting this replaces would have minted a bogus
    // type variable for an `E!` parameter.
    let combined: Vec<crate::ast::TypeParam> = impl_type_params
        .iter()
        .chain(method.type_params.iter())
        .cloned()
        .collect();
    let scope = super::ability_vars::generic_scope(infer, &combined);

    // Resolve the signature with the method's ability variables in scope, so
    // `with E` positions bind to the row variable. `report_type_misuse` is
    // false here: an `E`-used-as-a-type mistake reports once, from the body
    // check (this same resolution runs twice, scheme and body).
    let (params, ret, abilities) =
        infer.with_ability_var_scope(scope.ability_var_map.clone(), false, |infer| {
            let self_ty = substitute_type_params(for_type, &scope.type_var_map);

            let mut params = Vec::new();
            if method.has_self {
                params.push(self_ty);
            }
            for p in &method.params {
                let ty = match &p.ty {
                    Some(ty) => resolve_signature_type(infer, ty, for_type, &scope.type_var_map),
                    None => infer.fresh(),
                };
                params.push(ty);
            }

            let ret = if let Some(ty) = &method.ret_ty {
                resolve_signature_type(infer, ty, for_type, &scope.type_var_map)
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

            // The declared `with` clause. Bare names that name an ability
            // variable form the row tail; other names resolve concrete. An
            // absent clause means "pure", like a public function.
            let abilities = super::ability_vars::resolve_declared_with(
                infer,
                &method.abilities,
                &scope.ability_var_map,
                &method.name,
            );
            (params, ret, abilities)
        });

    let fn_ty = Type::function_with_abilities(params, ret, abilities);
    let scheme = if scope.is_empty() {
        Scheme::mono(fn_ty)
    } else {
        Scheme::poly_with_abilities(
            scope.quantified_type_vars.clone(),
            scope.ability_vars.clone(),
            fn_ty,
        )
    };

    super::locals::attach_scheme_bounds(
        infer,
        scheme,
        &combined,
        &scope.type_var_map,
        lenient_bounds,
    )
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
    let impl_type_params_ast = impl_def.type_params.clone();

    for method in &mut impl_def.methods {
        if method.resolved_symbol.is_none() {
            // Registration rejected the whole impl (invalid target); the
            // error is already reported.
            continue;
        }

        infer.reset_abilities();

        // Ordinary type parameters are rigid in the body (`T` → `Type::Param`);
        // ability (row) variables are not types, so they are excluded here and
        // installed as an ability-variable scope instead — exactly as an
        // ordinary function body treats its own `E!` (see
        // `check_function_body`). Bodies allocate their own fresh row
        // variables, distinct from the scheme's.
        let rigid: Vec<Arc<str>> = impl_type_params_ast
            .iter()
            .chain(method.type_params.iter())
            .filter(|tp| !tp.is_ability)
            .map(|tp| Arc::clone(&tp.name))
            .collect();
        // Bounds in the combined impl-then-method order — the same order
        // the scheme quantified them and the compiler allocates the
        // method's dictionary parameters. Unknown-trait errors were already
        // reported when the scheme was built; swallow the duplicates here.
        let combined_params: Vec<crate::ast::TypeParam> = impl_type_params_ast
            .iter()
            .chain(method.type_params.iter())
            .cloned()
            .collect();
        let ability_scope = super::ability_vars::ability_var_scope(infer, &combined_params);
        let bounds = infer.resolve_bound_params(&combined_params, &mut Vec::new());
        infer.with_ability_var_scope(ability_scope, true, |infer| {
            infer.with_rigid_params(rigid, |infer| {
                infer.with_bound_params(bounds, |infer| {
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
                                    errors.push(e.with_context(format!(
                                        "in inherent method `{}`",
                                        method.name
                                    )));
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
                            errors.push(
                                e.with_context(format!("in inherent method `{}`", method.name)),
                            );
                        }
                    }

                    // Solve the bound constraints this body recorded and finalize
                    // its dictionary annotations for the compiler.
                    infer.finish_body_constraints(&mut method.body, errors);
                });
            });
        });
    }
}

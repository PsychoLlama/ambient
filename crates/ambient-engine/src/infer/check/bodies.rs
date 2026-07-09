//! Function and const body checking (Phase 3) and ability enforcement
//! (Phase 4).

use std::sync::Arc;

use crate::fqn::ModuleId;
use crate::types::{AbilityId, AbilitySet, Type};

use crate::infer::Infer;
use crate::infer::env::TypeEnv;
use crate::infer::error::{BoxedTypeError, BoxedTypeErrorExt, TypeError, TypeErrorKind};

use super::locals::{own_item_scheme, resolve_erroring};

/// Type-check one function body (Phase 3).
///
/// The function's type parameters are rigid inside its body (and every
/// lambda/`let` nested in it): a written `T` annotation resolves to
/// `Type::Param("T")`, not an unresolved nominal reference. On success the
/// body's inferred abilities are recorded (by item index) for deferred
/// enforcement in Phase 4.
pub(super) fn check_function_body(
    infer: &mut Infer,
    func: &mut crate::ast::FunctionDef,
    idx: usize,
    env: &TypeEnv,
    current_module_id: Option<&ModuleId>,
    errors: &mut Vec<BoxedTypeError>,
    inferred_abilities: &mut Vec<(usize, AbilitySet)>,
) {
    infer.reset_abilities();

    let rigid: Vec<Arc<str>> = func
        .type_params
        .iter()
        .map(|tp| Arc::clone(&tp.name))
        .collect();
    infer.with_rigid_params(rigid, |infer| {
        let mut func_env = env.extend();
        let expected_ret_ty = func.ret_ty.clone().map(|ty| resolve_erroring(infer, &ty));

        for param in &func.params {
            let param_ty = match &param.ty {
                Some(ty) => resolve_erroring(infer, ty),
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

                bind_inferred_abilities(infer, env, func, current_module_id);
                inferred_abilities.push((idx, infer.current_abilities().clone()));
            }
            Err(e) => {
                errors.push(e.with_context(format!("in function `{}`", func.name)));
            }
        }
    });
}
/// Type-check every default implementation of one ability declaration
/// (Phase 3).
///
/// A method body is an ordinary function body: parameters bind at their
/// declared types, the result unifies with the declared return type, and
/// the method's type parameters are rigid inside it. Its *allowed* effects
/// are exactly the ability's declared `with`-dependencies (none means
/// pure) — recorded as a deferred subset check like inherent methods. That
/// rule is also what makes method identity well-founded: a body can never
/// perform its own ability (the ability is not in its own dependency row),
/// so a method's implementation hash never depends on itself.
pub(super) fn check_ability_method_bodies(
    infer: &mut Infer,
    def: &mut crate::ast::AbilityDef,
    env: &TypeEnv,
    errors: &mut Vec<BoxedTypeError>,
    deferred: &mut Vec<DeferredAbilityCheck>,
) {
    for method in &mut def.methods {
        let Some(body) = method.body.as_mut() else {
            // Body-less methods are either the Exception carve-out or
            // already reported by `register_abilities`.
            continue;
        };
        infer.reset_abilities();

        let rigid: Vec<Arc<str>> = method
            .type_params
            .iter()
            .map(|tp| Arc::clone(&tp.name))
            .collect();
        infer.with_rigid_params(rigid, |infer| {
            let mut method_env = env.extend();
            let expected_ret = resolve_erroring(infer, &method.ret_ty);

            for param in &method.params {
                let param_ty = match &param.ty {
                    Some(ty) => resolve_erroring(infer, ty),
                    None => infer.fresh(),
                };
                method_env.insert_mono(param.id, Arc::clone(&param.name), param_ty);
            }

            match infer.infer_expr(&method_env, body) {
                Ok(body_ty) => {
                    let span = (body.span.start, body.span.end);
                    if let Err(e) = infer.unify(&expected_ret, &body_ty, span) {
                        errors.push(e.with_context(format!(
                            "in ability method `{}::{}`: default implementation \
                             must return the declared type",
                            def.name, method.name
                        )));
                    }
                    deferred.push(DeferredAbilityCheck {
                        context: format!(
                            "default implementation of `{}::{}`",
                            def.name, method.name
                        ),
                        declared: def.dependencies.clone(),
                        inferred: infer.current_abilities().clone(),
                        span: method.span,
                    });
                }
                Err(e) => {
                    errors.push(e.with_context(format!(
                        "in ability method `{}::{}`",
                        def.name, method.name
                    )));
                }
            }
        });
    }
}
/// Check one `const` body: enforce that the initializer is a literal and
/// that its type matches the annotation.
pub(super) fn check_const_body(
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

    match infer.infer_expr(env, &mut const_def.value) {
        Ok(actual_ty) => {
            // With an explicit annotation, the value must match it. Without
            // one, the literal's own type is authoritative (registered in
            // Phase 1 via `const_declared_type`), so there is nothing to
            // reconcile.
            if let Some(annotation) = &const_def.ty {
                let expected_ty = resolve_erroring(infer, annotation);
                let span = (const_def.value.span.start, const_def.value.span.end);
                if let Err(e) = infer.unify(&expected_ty, &actual_ty, span) {
                    errors.push(
                        e.with_context(format!("in constant `{}`: type mismatch", const_def.name)),
                    );
                }
            }
        }
        Err(e) => {
            errors.push(e.with_context(format!("in constant `{}`", const_def.name)));
        }
    }
}
/// Bind an unannotated private function's ability variable to its body's
/// inferred effects, making the real effect set visible at call sites.
///
/// Annotated and public functions are skipped: their scheme carries the
/// declared (possibly empty) ability set, which enforcement checks instead.
fn bind_inferred_abilities(
    infer: &mut Infer,
    env: &TypeEnv,
    func: &crate::ast::FunctionDef,
    module_id: Option<&ModuleId>,
) {
    if !func.abilities.is_empty() || func.is_public {
        return;
    }
    let Some(scheme) = own_item_scheme(env, module_id, &func.name) else {
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
/// A recorded "body inferred these abilities against this declaration"
/// check, deferred until all bodies are checked (phase 4) so ability
/// variables bound late still resolve.
pub(super) struct DeferredAbilityCheck {
    /// Human-readable owner for error context, e.g. "inherent method `map`".
    pub(super) context: String,
    pub(super) declared: Vec<crate::ast::QualifiedName>,
    pub(super) inferred: AbilitySet,
    pub(super) span: crate::ast::Span,
}
/// Verify that a function's inferred abilities are a subset of its declared
/// abilities.
///
/// Applies to annotated functions and to public functions (where no `with`
/// clause means "pure"). Unannotated private functions are skipped — their
/// abilities are inferred, not declared. Abilities that remain polymorphic
/// after substitution are not enforced; they are constrained at the call
/// sites that instantiate them.
pub(super) fn enforce_declared_abilities(
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
/// Verify that inferred abilities are a subset of the declared clause
/// (no clause means pure). Shared by function and inherent-method
/// enforcement.
pub(super) fn enforce_ability_subset(
    infer: &Infer,
    context: &str,
    declared: &[crate::ast::QualifiedName],
    inferred: &AbilitySet,
    span: (u32, u32),
    errors: &mut Vec<BoxedTypeError>,
) {
    let inferred = infer.apply_abilities(inferred);

    // Namespace-aware resolution first (a `with core::system::Stdio` clause
    // must mean the system ability even when a local declaration
    // shadows the bare name), then a deliberately lenient bare fallback:
    // the namespace policy was already enforced where the clause was
    // resolved into the scheme (`build_function_scheme`,
    // `resolve_declared_abilities`), which reported
    // `AbilityRequiresNamespace` for a bare system name. Resolving that
    // name leniently here keeps the reported error from cascading into a
    // second "uses ability but doesn't declare it" error.
    let declared: Vec<AbilityId> = declared
        .iter()
        .filter_map(|qn| {
            infer
                .ability_resolver
                .resolve_ref(infer.ability_namespace(qn).as_ref(), qn.resolved_name())
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

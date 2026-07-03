//! Effect expression type inference.
//!
//! This module handles type inference for effect-related expressions:
//! - Perform: Immediately execute an ability operation
//! - Handle: Install handlers for abilities
//!
//! These are extracted from the main `infer_expr` function to improve
//! code organization.

use super::{Infer, InferResult, TypeEnv, TypeErrorKind, type_error};
use crate::ast::{AbilityCall, HandleExpr};
use crate::types::{AbilityId, AbilitySet, Type};

impl Infer {
    /// Infer the type of a perform expression.
    ///
    /// Perform immediately executes an ability operation and returns its result.
    /// This adds the ability to the current requirements.
    pub(super) fn infer_perform(
        &mut self,
        env: &TypeEnv,
        ability_call: &mut AbilityCall,
        span: (u32, u32),
    ) -> InferResult<Type> {
        // Infer types of arguments — on the real nodes, not clones:
        // inference records resolutions in the AST (trait method symbols,
        // operator overloads, module-call rewrites) that the compiler
        // depends on.
        let mut arg_tys = Vec::with_capacity(ability_call.args.len());
        for arg in &mut ability_call.args {
            arg_tys.push(self.infer_expr(env, arg)?);
        }

        // Look up the ability and method to get return type and additional abilities
        let (ability_id, result_ty, additional_abilities) = self.lookup_ability_method(
            &ability_call.ability,
            &ability_call.method,
            &arg_tys,
            span,
        )?;

        // Add primary ability to current requirements
        self.require_ability(ability_id);

        // Add any additional abilities (e.g., declared dependencies of module abilities)
        self.require_abilities(&additional_abilities);

        Ok(result_ty)
    }

    /// Infer the type of a handle expression.
    ///
    /// Handle installs ability handlers and evaluates the body.
    /// The result type is the type of the body expression.
    ///
    /// Effects split three ways:
    /// - the body's effects, minus what the handlers cover, flow to the
    ///   enclosing context (deferred when the body's set is still
    ///   polymorphic — see [`crate::infer::PendingDischarge`]);
    /// - the handler arms' and else clause's own effects always flow to
    ///   the enclosing context (they run outside the delimited body);
    /// - handler *values* contribute nothing here — their arms were
    ///   checked where the literal was written.
    pub(super) fn infer_handle(
        &mut self,
        env: &TypeEnv,
        handle_expr: &mut HandleExpr,
        span: (u32, u32),
    ) -> InferResult<Type> {
        // Infer the body with a clean effect accumulator. Mutations matter:
        // inference records resolutions (trait methods, rewrites, types)
        // that the compiler reads, so it must run on real nodes, never
        // clones.
        let (body_result, body_abilities) =
            self.with_isolated_effects(|infer| infer.infer_expr(env, &mut handle_expr.body));
        // Everything below (handler values, else clause, arm bodies) runs
        // outside the delimited body, so its effects accumulate for the
        // enclosing context, on top of the restored set.
        let body_ty = body_result?;

        // Collect handled abilities from handler values (from `with` clause).
        // The inferred `Handler<A>` type must land on the real expression:
        // the compiler decides whether to emit HandleWithValue based on it.
        let mut handled_abilities = Vec::new();
        for handler_value in &mut handle_expr.handler_values {
            let handler_ty = self.infer_expr(env, handler_value)?;

            // Handler value should be Handler<A>
            if let Type::Handler(handler_type) = handler_ty {
                handled_abilities.push(handler_type.ability);
            } else {
                return Err(type_error(
                    TypeErrorKind::TypeMismatch {
                        expected: Type::handler(AbilityId::from_bytes([0; 32])), // Generic Handler type
                        actual: handler_ty,
                    },
                    (handler_value.span.start, handler_value.span.end),
                ));
            }
        }

        // The handle expression's result type. Without an else clause it is
        // the body's type; with one, it is the else transform's return type
        // (`else` is `(result) => expr`, applied on normal completion).
        let result_ty = if let Some(else_clause) = &mut handle_expr.else_clause {
            let else_ty = self.infer_expr(env, else_clause)?;
            let handle_ty = self.fresh();
            let effects = self.fresh_ability_var();
            let expected =
                Type::function_with_abilities(vec![body_ty.clone()], handle_ty.clone(), effects);
            self.unify(&expected, &else_ty, span)?;
            handle_ty
        } else {
            body_ty.clone()
        };

        // Check inline handler arms against the ability's declared method
        // signature: parameters take the declared types, `resume` feeds the
        // method's return type, and the arm's value becomes the handle
        // expression's result (arms bypass the else transform).
        for handler in &mut handle_expr.handlers {
            let ability_id = self.check_handler_arm(env, handler, &result_ty, span)?;
            handled_abilities.push(ability_id);
        }

        // Compute the body's remaining (unhandled) abilities and require
        // them from the enclosing context.
        let remaining_abilities = match self.apply_abilities(&body_abilities) {
            AbilitySet::Empty => AbilitySet::Empty,
            AbilitySet::Concrete(abilities) => {
                let remaining: Vec<_> = abilities
                    .iter()
                    .filter(|a| !handled_abilities.contains(a))
                    .copied()
                    .collect();
                AbilitySet::from_abilities(remaining)
            }
            body @ (AbilitySet::Var(_) | AbilitySet::Row { .. } | AbilitySet::Unresolved(_)) => {
                // The body's effects are still polymorphic — typically
                // calls to functions whose inferred effects bind later in
                // the check pass. Contribute a remainder variable now and
                // defer the subtraction until all bodies are checked.
                let remainder = self.r#gen.fresh_ability_id();
                self.pending_discharges
                    .push(crate::infer::PendingDischarge {
                        body,
                        handled: handled_abilities,
                        remainder,
                    });
                AbilitySet::Var(remainder)
            }
        };
        self.require_abilities(&remaining_abilities);

        Ok(result_ty)
    }

    /// Check one inline handler arm against the ability's declared method
    /// signature: parameters take the declared types, `resume` feeds the
    /// method's return type, and the arm's value unifies with the handle
    /// expression's result. Returns the handled ability's identity.
    fn check_handler_arm(
        &mut self,
        env: &TypeEnv,
        handler: &mut crate::ast::Handler,
        result_ty: &Type,
        span: (u32, u32),
    ) -> InferResult<AbilityId> {
        let handler_span = (handler.span.start, handler.span.end);
        let Some(ability_id) = self.ability_name_to_id(&handler.ability.name) else {
            return Err(type_error(
                TypeErrorKind::UnknownAbility {
                    name: handler.ability.name.clone(),
                },
                handler_span,
            ));
        };

        let Some((param_tys, ret_ty)) = self.ability_method_signature(ability_id, &handler.method)
        else {
            return Err(type_error(
                TypeErrorKind::UnknownAbilityMethod {
                    ability: handler.ability.name.clone(),
                    method: handler.method.clone(),
                },
                handler_span,
            ));
        };
        if handler.params.len() != param_tys.len() {
            return Err(type_error(
                TypeErrorKind::HandlerMethodArityMismatch {
                    ability: handler.ability.name.clone(),
                    method: handler.method.clone(),
                    expected: param_tys.len(),
                    actual: handler.params.len(),
                },
                handler_span,
            ));
        }

        let mut handler_env = env.extend();
        for (param, declared_ty) in handler.params.iter().zip(&param_tys) {
            let param_ty = match &param.ty {
                Some(ty) => {
                    let annotated = self.resolve_holes(ty);
                    self.unify(declared_ty, &annotated, handler_span)?;
                    annotated
                }
                None => declared_ty.clone(),
            };
            handler_env.insert_mono(param.id, param.name.clone(), param_ty);
        }

        // A `!`-returning method (Exception::throw) has no statically
        // knowable resume type: the host can raise it at any perform
        // site, and resuming substitutes a value for the failing call.
        let value_ty = match self.apply(&ret_ty) {
            Type::Never => None,
            ret => Some(ret),
        };
        self.resume_contexts.push(crate::infer::ResumeContext {
            value_ty,
            result_ty: Some(result_ty.clone()),
        });
        let handler_result = self.infer_expr(&handler_env, &mut handler.body);
        self.resume_contexts.pop();
        let handler_ty = handler_result?;
        self.unify(result_ty, &handler_ty, span)?;
        Ok(ability_id)
    }
}

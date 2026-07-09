//! Effect expression type inference.
//!
//! This module handles type inference for effect-related expressions:
//! - Perform: Immediately execute an ability operation
//! - Handle: Install handlers for abilities
//!
//! These are extracted from the main `infer_expr` function to improve
//! code organization.

use super::{Infer, InferResult, TypeEnv, TypeErrorKind, type_error};
use crate::ast::{AbilityCall, Expr, ExprKind, HandleExpr, HandlerLiteralMethod};
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
        // Everything below (handlers, else clause, arm bodies) runs outside
        // the delimited body, so its effects accumulate for the enclosing
        // context, on top of the restored set.
        let body_ty = body_result?;

        // The handle expression's result type `R_handle`. Without an else
        // clause it is the body's type; with one, it is the else transform's
        // return type (`else` is `(result) => expr`, applied on normal
        // completion).
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

        // Walk the flat handler list. A handler-literal node is checked
        // in-place (arms may span abilities here, at the install site); any
        // other expression must be a `Handler<A, R>` value whose answer type
        // unifies with `R_handle` (this closes the latent soundness hole).
        let mut handled_abilities = Vec::new();
        for handler in &mut handle_expr.handlers {
            self.check_installed_handler(env, handler, &result_ty, span, &mut handled_abilities)?;
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

    /// Check one installed handler in a `with` list against the handle
    /// result type `result_ty` (`R_handle`), collecting the ability id(s) it
    /// covers into `handled`.
    ///
    /// A handler-literal node is checked arm-by-arm in place (arms may span
    /// abilities at an install site). Any other expression must infer to a
    /// `Handler<A, R>` value; its answer type `R` unifies with `R_handle`.
    fn check_installed_handler(
        &mut self,
        env: &TypeEnv,
        handler: &mut Expr,
        result_ty: &Type,
        span: (u32, u32),
        covered: &mut Vec<AbilityId>,
    ) -> InferResult<()> {
        if matches!(&handler.kind, ExprKind::HandlerLiteral(_)) {
            let ExprKind::HandlerLiteral(lit) = &mut handler.kind else {
                unreachable!("guarded by matches! above")
            };
            // Take the arms out so the borrow of `handler.kind` ends and we
            // can `&mut self` freely while iterating.
            let mut methods = std::mem::take(&mut lit.methods);
            let mut result = Ok(());
            for method in &mut methods {
                match self.check_handler_arm(env, method, result_ty, span) {
                    Ok(ability_id) => covered.push(ability_id),
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
            }
            if let ExprKind::HandlerLiteral(lit) = &mut handler.kind {
                lit.methods = methods;
            }
            // The inline multi-ability literal has no single handler type; the
            // compiler dispatches on the node directly, so a placeholder ty is
            // fine.
            handler.ty = Some(Type::Unit);
            result
        } else {
            let handler_ty = self.infer_expr(env, handler)?;
            match self.apply(&handler_ty) {
                Type::Handler(ht) => {
                    self.unify(&ht.answer, result_ty, span)?;
                    covered.push(ht.ability);
                    Ok(())
                }
                actual => Err(type_error(
                    TypeErrorKind::TypeMismatch {
                        expected: Type::handler(AbilityId::from_bytes([0; 32]), result_ty.clone()),
                        actual,
                    },
                    (handler.span.start, handler.span.end),
                )),
            }
        }
    }

    /// Check one handler-literal arm against the ability's declared method
    /// signature: parameters take the declared types, `resume` feeds the
    /// method's return type, and the arm's value unifies with `result_ty`
    /// (the answer type — the handle result when installed inline, the
    /// handler value's own answer var when standalone). Returns the handled
    /// ability's identity.
    pub(super) fn check_handler_arm(
        &mut self,
        env: &TypeEnv,
        handler: &mut HandlerLiteralMethod,
        result_ty: &Type,
        span: (u32, u32),
    ) -> InferResult<AbilityId> {
        let handler_span = (handler.span.start, handler.span.end);
        // Same namespace policy as performs: platform abilities must be
        // handled as `core::system::Stdio::out(...)`, locals and the
        // builtin Exception bare.
        let ability_id = self.resolve_ability_ref(&handler.ability, handler_span)?;

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

        // `resume(v)` always feeds the method's declared return type — no
        // exceptions. `Exception::throw` returns `!` (never), which nothing
        // unifies with, so an Exception arm is catch-only: resuming a throw
        // is a type error, not a way to substitute a value for a failing
        // call. Fallible host operations return `Result` and are matched on
        // instead of resumed. (Previously a `!` return made resume
        // unconstrained; that resume-with-substitute escape hatch is gone.)
        self.resume_contexts.push(crate::infer::ResumeContext {
            value_ty: Some(self.apply(&ret_ty)),
            result_ty: Some(result_ty.clone()),
        });
        let handler_result = self.infer_expr(&handler_env, &mut handler.body);
        self.resume_contexts.pop();
        let handler_ty = handler_result?;
        self.unify(result_ty, &handler_ty, span)?;
        Ok(ability_id)
    }
}

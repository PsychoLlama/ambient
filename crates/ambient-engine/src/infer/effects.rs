//! Effect expression type inference.
//!
//! This module handles type inference for effect-related expressions:
//! - Perform: Immediately execute an ability operation
//! - Suspend: Create a suspended ability value
//! - Handle: Install handlers for abilities
//!
//! These are extracted from the main `infer_expr` function to improve
//! code organization.

use super::{type_error, Infer, InferResult, TypeEnv, TypeErrorKind};
use crate::ast::{AbilityCall, HandleExpr};
use crate::types::{AbilitySet, Type};

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
        // Infer types of arguments
        let mut arg_tys = Vec::with_capacity(ability_call.args.len());
        for arg in &mut ability_call.args.clone() {
            let mut arg_clone = arg.clone();
            arg_tys.push(self.infer_expr(env, &mut arg_clone)?);
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

        // Add any additional abilities (e.g., underlying abilities from Async.all/race)
        self.require_abilities(&additional_abilities);

        Ok(result_ty)
    }

    /// Infer the type of a suspend expression.
    ///
    /// Suspend creates a suspended ability value without executing it.
    /// Returns `Ability<result_ty, ability_id>`.
    pub(super) fn infer_suspend(
        &mut self,
        env: &TypeEnv,
        ability_call: &mut AbilityCall,
        span: (u32, u32),
    ) -> InferResult<Type> {
        // Infer types of arguments
        let mut arg_tys = Vec::with_capacity(ability_call.args.len());
        for arg in &mut ability_call.args.clone() {
            let mut arg_clone = arg.clone();
            arg_tys.push(self.infer_expr(env, &mut arg_clone)?);
        }

        // Look up the ability and method to get return type
        // Note: For suspend, we ignore additional_abilities since we're just creating a value
        let (ability_id, result_ty, _additional_abilities) = self.lookup_ability_method(
            &ability_call.ability,
            &ability_call.method,
            &arg_tys,
            span,
        )?;

        // Return Ability<result_ty, ability_id> - a suspended ability value
        Ok(Type::ability_value(
            result_ty,
            AbilitySet::single(ability_id),
        ))
    }

    /// Infer the type of a handle expression.
    ///
    /// Handle installs ability handlers and evaluates the body.
    /// The result type is the type of the body expression.
    pub(super) fn infer_handle(
        &mut self,
        env: &TypeEnv,
        handle_expr: &mut HandleExpr,
        span: (u32, u32),
    ) -> InferResult<Type> {
        // Save current abilities
        let saved_abilities = self.current_abilities.clone();
        self.reset_abilities();

        // Infer the body expression
        let body_ty = self.infer_expr(env, &mut handle_expr.body.clone())?;

        // Get the abilities required by the body
        let body_abilities = self.current_abilities.clone();

        // Collect handled abilities from handler values (from `with` clause)
        let mut handled_abilities = Vec::new();
        for handler_value in &mut handle_expr.handler_values.clone() {
            let handler_ty = self.infer_expr(env, &mut handler_value.clone())?;

            // Handler value should be Handler<A>
            if let Type::Handler(handler_type) = handler_ty {
                handled_abilities.push(handler_type.ability);
            } else {
                return Err(type_error(
                    TypeErrorKind::TypeMismatch {
                        expected: Type::handler(0), // Generic Handler type
                        actual: handler_ty,
                    },
                    (handler_value.span.start, handler_value.span.end),
                ));
            }
        }

        // Collect handled abilities from inline handlers
        for handler in &handle_expr.handlers {
            if let Some(ability_id) = self.ability_name_to_id(&handler.ability.name) {
                handled_abilities.push(ability_id);
            }

            // Infer handler body (with continuation parameter)
            let mut handler_env = env.extend();
            for param in &handler.params {
                let param_ty = param.ty.clone().unwrap_or_else(|| self.fresh());
                handler_env.insert_mono(param.id, param.name.clone(), param_ty);
            }

            // Handler body should produce the same type as the overall handle expression
            // (when resume is called, it continues with the body's result type)
            let mut handler_body = handler.body.clone();
            let handler_ty = self.infer_expr(&handler_env, &mut handler_body)?;
            self.unify(&body_ty, &handler_ty, span)?;
        }

        // Compute remaining unhandled abilities
        let remaining_abilities = match &body_abilities {
            AbilitySet::Empty => AbilitySet::Empty,
            AbilitySet::Concrete(abilities) => {
                let remaining: Vec<_> = abilities
                    .iter()
                    .filter(|a| !handled_abilities.contains(a))
                    .copied()
                    .collect();
                AbilitySet::from_abilities(remaining)
            }
            AbilitySet::Var(_) | AbilitySet::Row { .. } | AbilitySet::Unresolved(_) => {
                // Can't statically determine which abilities are handled
                // For now, assume all handlers match
                body_abilities.clone()
            }
        };

        // Restore saved abilities and add remaining
        self.current_abilities = saved_abilities.union(&remaining_abilities);

        // Handle else clause if present
        if let Some(else_clause) = &mut handle_expr.else_clause.clone() {
            let else_ty = self.infer_expr(env, else_clause)?;
            self.unify(&body_ty, &else_ty, span)?;
        }

        Ok(body_ty)
    }
}

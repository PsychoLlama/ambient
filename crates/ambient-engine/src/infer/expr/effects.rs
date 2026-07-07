//! Inference for handler-literal and sandbox expressions.

use std::sync::Arc;

use crate::infer::{Infer, InferResult, TypeEnv, TypeErrorKind, type_error};
use crate::types::{AbilityId, AbilitySet, Type};

impl Infer {
    /// Infer the type of a standalone handler literal expression (a
    /// `Handler<A, R>` value — e.g. `let mock_fs = { … }`).
    ///
    /// A handler value covers exactly one ability, derived from its arms'
    /// qualified prefixes (arms spanning multiple abilities are a type error
    /// — use the literal inline, or split it). Each arm is checked against
    /// the ability's declared signature with `result_ty` a fresh answer var
    /// `R`, so a non-resuming arm's return type constrains `R`. `R` is
    /// generalizable, so an always-resuming handler stays `∀R. Handler<A, R>`
    /// and instantiates per use site.
    pub(super) fn infer_handler_literal(
        &mut self,
        env: &TypeEnv,
        handler_lit: &mut crate::ast::HandlerLiteralExpr,
        span: (u32, u32),
    ) -> InferResult<Type> {
        use std::sync::Arc;

        if handler_lit.methods.is_empty() {
            return Err(type_error(TypeErrorKind::HandlerEmpty, span));
        }

        // The answer type `R`: what an arm yields when it returns without
        // resuming. Fresh (and generalizable) so a resuming-only handler
        // stays polymorphic in `R`.
        let answer = self.fresh();

        let mut ability_id: Option<AbilityId> = None;
        let mut ability_names: Vec<Arc<str>> = Vec::new();
        for method in &mut handler_lit.methods {
            let arm_ability = self.check_handler_arm(env, method, &answer, span)?;
            match ability_id {
                None => {
                    ability_id = Some(arm_ability);
                    ability_names.push(Arc::clone(&method.ability.name));
                }
                Some(existing) if existing != arm_ability => {
                    let name = Arc::clone(&method.ability.name);
                    if !ability_names.contains(&name) {
                        ability_names.push(name);
                    }
                }
                Some(_) => {}
            }
        }

        if ability_names.len() > 1 {
            return Err(type_error(
                TypeErrorKind::HandlerValueMultipleAbilities {
                    abilities: ability_names,
                },
                span,
            ));
        }

        // `methods` is non-empty (guarded above), so the first arm set the
        // ability; treat an absent ability as the empty case for safety.
        let Some(ability_id) = ability_id else {
            return Err(type_error(TypeErrorKind::HandlerEmpty, span));
        };
        // Apply the substitution so a non-resuming arm's constraint on the
        // answer type is reflected in the returned handler type.
        Ok(Type::handler(ability_id, self.apply(&answer)))
    }

    /// Infer the type of a sandbox expression.
    ///
    /// A sandbox restricts the abilities the body may *use* to the allowed
    /// list. It installs no handlers — the body executes against the
    /// enclosing context's handlers (the compiler emits the body directly)
    /// — so the body's effects still flow to the enclosing context. The
    /// restriction check runs here when the body's effect set is already
    /// concrete, and is deferred to the end of the module check otherwise
    /// (calls to functions whose effects bind later).
    pub(super) fn infer_sandbox(
        &mut self,
        env: &TypeEnv,
        sandbox_expr: &mut crate::ast::SandboxExpr,
        span: (u32, u32),
    ) -> InferResult<Type> {
        // Resolve the allowed list; unknown names are errors, and the
        // namespace policy applies: `sandbox with core::system::Log { ... }`.
        let mut allowed_ability_ids = Vec::with_capacity(sandbox_expr.allowed_abilities.len());
        for ability_name in &sandbox_expr.allowed_abilities {
            allowed_ability_ids.push(self.resolve_ability_ref(ability_name, span)?);
        }

        // Infer the body with a clean accumulator so its effect set can be
        // inspected in isolation — on the real node, not a clone:
        // inference records resolutions (trait method symbols, module-call
        // rewrites, expression types) that the compiler depends on.
        let (body_result, body_abilities) =
            self.with_isolated_effects(|infer| infer.infer_expr(env, &mut sandbox_expr.body));
        let body_ty = body_result?;

        // Enforce the restriction now if the body's effects are already
        // concrete; otherwise defer until every body is checked.
        let allowed_names: Vec<Arc<str>> = sandbox_expr
            .allowed_abilities
            .iter()
            .map(|n| n.name.clone())
            .collect();
        let applied = self.apply_abilities(&body_abilities);
        match &applied {
            AbilitySet::Empty | AbilitySet::Concrete(_) => {
                if let Some(err) = self.sandbox_violation(
                    applied.concrete_abilities(),
                    &allowed_ability_ids,
                    &allowed_names,
                    span,
                ) {
                    return Err(err);
                }
            }
            _ => self
                .pending_sandbox_checks
                .push(crate::infer::PendingSandboxCheck {
                    body: applied.clone(),
                    allowed: allowed_ability_ids,
                    allowed_names,
                    span,
                }),
        }

        // The sandbox installs no handlers, so the body's effects are the
        // enclosing context's problem exactly as if the sandbox weren't
        // there.
        self.require_abilities(&applied);

        Ok(body_ty)
    }

    /// The error for the first body ability outside the allowed list, if
    /// any.
    pub(crate) fn sandbox_violation(
        &self,
        used: &[crate::types::AbilityId],
        allowed: &[crate::types::AbilityId],
        allowed_names: &[Arc<str>],
        span: (u32, u32),
    ) -> Option<crate::infer::error::BoxedTypeError> {
        let violation = used.iter().find(|a| !allowed.contains(a))?;
        let ability_name = self.ability_id_to_name(*violation).unwrap_or("unknown");
        Some(type_error(
            TypeErrorKind::SandboxAbilityViolation {
                ability: ability_name.into(),
                allowed: allowed_names.to_vec(),
            },
            span,
        ))
    }
}

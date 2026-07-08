//! Intrinsic type inference for built-in functions.
//!
//! Signatures come from the shared intrinsic table
//! (`crate::compiler::intrinsics`) — the same table the compiler emits
//! bytecode from — so an intrinsic cannot type-check without compiling or
//! vice versa.

use super::error::BoxedTypeErrorExt;
use super::{Infer, InferResult, TypeEnv, TypeErrorKind, type_error};
use crate::ast::Expr;
use crate::types::Type;

impl Infer {
    /// Try to infer the type of an intrinsic function call.
    ///
    /// Returns `Some(return_type)` if the name is a known intrinsic,
    /// `None` if it should be handled as a regular function call.
    ///
    /// # Errors
    ///
    /// Returns a `TypeError` if argument types don't match the intrinsic's
    /// signature, or if the argument count is wrong.
    pub fn try_infer_intrinsic(
        &mut self,
        env: &TypeEnv,
        qualified_name: &crate::ast::QualifiedName,
        args: &mut [Expr],
        span: (u32, u32),
    ) -> InferResult<Option<Type>> {
        // Match on the canonical target: `use core::primitives::number; Number::sqrt(x)`,
        // `use core::primitives::number::sqrt; sqrt(x)`, and a literal `core::primitives::number::sqrt(x)`
        // all resolve to the same intrinsic `Fqn`.
        let Some(fqn) = qualified_name.intrinsic_fqn() else {
            return Ok(None);
        };

        let Some(intrinsic) = crate::compiler::intrinsics::find(&fqn) else {
            return Ok(None);
        };

        // Human-readable spelling for diagnostics (never a lookup key).
        let display = fqn.to_string();
        let signature = intrinsic.signature(&mut self.r#gen);

        if args.len() != signature.params.len() {
            return Err(type_error(
                TypeErrorKind::ArityMismatch {
                    expected: signature.params.len(),
                    actual: args.len(),
                },
                span,
            )
            .with_context(format!("in call to `{display}`")));
        }

        for (i, (arg, param_ty)) in args.iter_mut().zip(&signature.params).enumerate() {
            let arg_ty = self.infer_expr(env, arg)?;
            if let Err(e) = self.unify(param_ty, &arg_ty, span) {
                return Err(e.with_context(format!("in argument {} of `{display}`", i + 1)));
            }
        }

        Ok(Some(self.apply(&signature.ret)))
    }
}

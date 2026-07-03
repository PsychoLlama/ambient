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
        let path: Vec<&str> = qualified_name.path.iter().map(AsRef::as_ref).collect();
        let name = qualified_name.name.as_ref();

        let Some(intrinsic) = crate::compiler::intrinsics::find(&path, name) else {
            return Ok(None);
        };

        let signature = intrinsic.signature(&mut self.r#gen);

        if args.len() != signature.params.len() {
            return Err(type_error(
                TypeErrorKind::ArityMismatch {
                    expected: signature.params.len(),
                    actual: args.len(),
                },
                span,
            )
            .with_context(format!("in call to `{}::{name}`", path.join("::"))));
        }

        for (i, (arg, param_ty)) in args.iter_mut().zip(&signature.params).enumerate() {
            let arg_ty = self.infer_expr(env, arg)?;
            if let Err(e) = self.unify(param_ty, &arg_ty, span) {
                return Err(e.with_context(format!(
                    "in argument {} of `{}::{name}`",
                    i + 1,
                    path.join("::")
                )));
            }
        }

        Ok(Some(self.apply(&signature.ret)))
    }
}

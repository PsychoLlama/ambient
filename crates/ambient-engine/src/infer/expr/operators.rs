//! Inference for binary operators: built-in primitives and trait-overloaded
//! operators on nominal types.

use std::sync::Arc;

use crate::ast::{BinaryOp, Expr};
use crate::infer::{Infer, InferResult, TypeEnv};
use crate::types::Type;

impl Infer {
    /// Infer the type of a binary operation.
    ///
    /// For primitive types, uses built-in operators.
    /// For nominal types, looks up the appropriate trait (Add, Eq, etc.).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn infer_binary(
        &mut self,
        env: &TypeEnv,
        op: BinaryOp,
        left: &mut Expr,
        right: &mut Expr,
        resolved_op: &mut Option<Arc<str>>,
        span: (u32, u32),
    ) -> InferResult<Type> {
        let left_ty = self.infer_expr(env, left)?;
        let right_ty = self.infer_expr(env, right)?;

        // Apply substitutions to get the actual types
        let left_ty = self.apply(&left_ty);
        let right_ty = self.apply(&right_ty);

        // Check for operator overloading on nominal types
        if let Type::Nominal(nominal) = &left_ty {
            // Get the trait and method name for this operator
            if let Some((trait_name, method_name)) = operator_trait(op) {
                // Look up the trait
                if let Some(trait_id) = self.trait_registry.lookup_trait(trait_name) {
                    // Check if the type implements this trait
                    let method_symbol = self
                        .trait_registry
                        .get_impl(trait_id, nominal.uuid)
                        .and_then(|impl_| impl_.methods.get(method_name).cloned());

                    if let Some(symbol) = method_symbol {
                        // Unify operands (both must be the same nominal type)
                        self.unify(&left_ty, &right_ty, span)?;

                        // Store the resolved dispatch symbol for compilation
                        *resolved_op = Some(symbol);

                        // Return type depends on the operator category
                        return Ok(operator_return_type(op, &left_ty));
                    }
                }
            }
        }

        // Built-in operators for primitive types
        match op {
            // Arithmetic operators: Number -> Number -> Number
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
                // Special case: Add also works for String concatenation
                if op == BinaryOp::Add && left_ty == Type::string() {
                    self.unify(&right_ty, &Type::string(), span)?;
                    return Ok(Type::string());
                }
                self.unify(&left_ty, &Type::number(), span)?;
                self.unify(&right_ty, &Type::number(), span)?;
                Ok(Type::number())
            }

            // Comparison operators: a -> a -> Bool
            BinaryOp::Eq | BinaryOp::Ne => {
                self.unify(&left_ty, &right_ty, span)?;
                Ok(Type::bool())
            }

            // Ordering operators: Number -> Number -> Bool
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                self.unify(&left_ty, &Type::number(), span)?;
                self.unify(&right_ty, &Type::number(), span)?;
                Ok(Type::bool())
            }

            // Logical operators: Bool -> Bool -> Bool
            BinaryOp::And | BinaryOp::Or => {
                self.unify(&left_ty, &Type::bool(), span)?;
                self.unify(&right_ty, &Type::bool(), span)?;
                Ok(Type::bool())
            }
        }
    }
}

/// Map binary operators to their corresponding trait and method names.
/// Returns `(trait_name, method_name)` if the operator can be overloaded.
fn operator_trait(op: BinaryOp) -> Option<(&'static str, &'static str)> {
    match op {
        BinaryOp::Add => Some(("Add", "add")),
        BinaryOp::Sub => Some(("Sub", "sub")),
        BinaryOp::Mul => Some(("Mul", "mul")),
        BinaryOp::Div => Some(("Div", "div")),
        BinaryOp::Mod => Some(("Mod", "rem")),
        BinaryOp::Eq | BinaryOp::Ne => Some(("Eq", "eq")),
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => Some(("Ord", "cmp")),
        // Logical operators cannot be overloaded
        BinaryOp::And | BinaryOp::Or => None,
    }
}

/// Get the return type for an overloaded operator.
fn operator_return_type(op: BinaryOp, operand_ty: &Type) -> Type {
    match op {
        // Arithmetic operators return the same type
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
            operand_ty.clone()
        }
        // Comparison operators return Bool
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            Type::bool()
        }
        // Logical operators (not overloadable, but included for completeness)
        BinaryOp::And | BinaryOp::Or => Type::bool(),
    }
}

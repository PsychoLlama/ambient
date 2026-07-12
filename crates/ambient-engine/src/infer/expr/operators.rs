//! Inference for binary operators: built-in primitives and trait-overloaded
//! operators on nominal types.

use std::sync::Arc;

use crate::ast::{BinaryOp, Expr, ResolvedMethod};
use crate::infer::error::{TypeError, TypeErrorKind};
use crate::infer::{Infer, InferResult, TypeEnv};
use crate::types::{ReservedTrait, Type};

impl Infer {
    /// Infer the type of a binary operation.
    ///
    /// For primitive types, uses built-in operators.
    /// For nominal types, looks up the appropriate trait (Add, Eq, etc.).
    /// For rigid type parameters, dispatches through a declared bound on
    /// the reserved operator trait (`T: Ord` enables `<` on `T`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn infer_binary(
        &mut self,
        env: &TypeEnv,
        op: BinaryOp,
        left: &mut Expr,
        right: &mut Expr,
        resolved_op: &mut Option<ResolvedMethod>,
        span: (u32, u32),
    ) -> InferResult<Type> {
        let left_ty = self.infer_expr(env, left)?;
        let right_ty = self.infer_expr(env, right)?;

        // Apply substitutions to get the actual types
        let left_ty = self.apply(&left_ty);
        let right_ty = self.apply(&right_ty);

        // Check for operator overloading on nominal types. Operators anchor
        // on the *reserved identity* of the prelude trait (`TRAIT_ADD_UUID`
        // etc.), never on whatever trait is lexically in scope under the
        // name — a user trait named `Add` can shadow the prelude's for
        // `use`/impl purposes, but it can never capture `+`.
        //
        // Primitives are exempt: their operators are always the builtins.
        // Core *does* implement the operator traits for primitives, but
        // those impls exist to serve as dictionary sources for bounded
        // generics — their bodies are the builtin operators, so routing a
        // concrete `1 + 2` through them would recurse forever.
        if let Some(type_uuid) =
            crate::infer::inherent::impl_key_for(&left_ty).and_then(|k| k.uuid())
            && left_ty.as_primitive().is_none()
            && let Some((op_trait, method_name)) = operator_trait(op)
        {
            // Check if the type implements the reserved operator trait. Any
            // nominal identity qualifies — a struct or a declared enum — so
            // `shape_a == shape_b` dispatches through the enum's `Eq` impl.
            let method_symbol = self
                .trait_registry
                .get_impl(op_trait.uuid(), type_uuid)
                .and_then(|impl_| impl_.methods.get(method_name).cloned());

            if let Some(symbol) = method_symbol {
                // Unify operands (both must be the same nominal type)
                self.unify(&left_ty, &right_ty, span)?;

                // Store the resolved dispatch symbol for compilation
                *resolved_op = Some(ResolvedMethod::Symbol(symbol));

                // Return type depends on the operator category
                return Ok(operator_return_type(op, &left_ty));
            }
        }

        // An operator on a rigid type parameter dispatches through the
        // parameter's bound on the reserved operator trait: `a == b` with
        // `a, b: T` requires `T: Eq` and compiles as a dictionary-slot
        // call, exactly like `a.eq(b)` would.
        if let Type::Param(param) = &left_ty
            && let Some((op_trait, method_name)) = operator_trait(op)
        {
            let Some(dict_index) = self.bound_param_index(param, op_trait.uuid()) else {
                return Err(Box::new(TypeError::new(
                    TypeErrorKind::MissingParamBound {
                        param: Arc::clone(param),
                        trait_name: Arc::from(op_trait.name()),
                    },
                    span,
                )));
            };
            let slot = self
                .trait_registry
                .get_trait(op_trait.uuid())
                .and_then(|def| def.dictionary_slot(method_name))
                .unwrap_or_default();

            self.unify(&left_ty, &right_ty, span)?;
            *resolved_op = Some(ResolvedMethod::DictSlot { dict_index, slot });
            return Ok(operator_return_type(op, &left_ty));
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

/// Map binary operators to their corresponding reserved trait and method
/// name. Returns `(trait, method_name)` if the operator can be overloaded.
pub(crate) fn operator_trait(op: BinaryOp) -> Option<(ReservedTrait, &'static str)> {
    match op {
        BinaryOp::Add => Some((ReservedTrait::Add, "add")),
        BinaryOp::Sub => Some((ReservedTrait::Sub, "sub")),
        BinaryOp::Mul => Some((ReservedTrait::Mul, "mul")),
        BinaryOp::Div => Some((ReservedTrait::Div, "div")),
        BinaryOp::Mod => Some((ReservedTrait::Mod, "rem")),
        BinaryOp::Eq | BinaryOp::Ne => Some((ReservedTrait::Eq, "eq")),
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            Some((ReservedTrait::Ord, "cmp"))
        }
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

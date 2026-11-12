//! Field access and operators.

use super::Checker;
use crate::core::{self, Primitive};
use crate::error::Error;
use crate::surface::{Expr, Operator, Reference};
use crate::types::Ty;

impl Checker<'_> {
    /// `value` at type `ty` if the type describes it, and `otherwise` if
    /// not.
    pub(super) fn cast(
        &mut self,
        value: &Expr<Reference>,
        ty: &Ty,
        otherwise: &Expr<Reference>,
    ) -> Result<(core::Expr, Ty), Error> {
        let (value, _) = self.infer(value)?;
        self.tested(ty)?;
        let target = self.env.normalize(ty);
        let (otherwise, alternative) = self.infer(otherwise)?;
        let core = core::Expr::Cast {
            value: Box::new(value),
            ty: self.lower(&target),
            otherwise: Box::new(otherwise),
        };
        Ok((core, self.env.union(vec![target, alternative])))
    }

    pub(super) fn binary(
        &mut self,
        operator: Operator,
        left: &Expr<Reference>,
        right: &Expr<Reference>,
    ) -> Result<(core::Expr, Ty), Error> {
        let (core_left, left) = self.infer(left)?;
        let (core_right, right) = self.infer(right)?;
        let both = |ty: &Ty| self.env.subtype(&left, ty) && self.env.subtype(&right, ty);
        let choice = match operator {
            Operator::Equal => Some((Primitive::Equal, Ty::Bool)),
            _ if both(&Ty::Int) => Some(match operator {
                Operator::Add => (Primitive::IntAdd, Ty::Int),
                Operator::Subtract => (Primitive::IntSubtract, Ty::Int),
                Operator::Multiply => (Primitive::IntMultiply, Ty::Int),
                Operator::Less | Operator::Equal => (Primitive::IntLess, Ty::Bool),
            }),
            _ if both(&Ty::Float) => Some(match operator {
                Operator::Add => (Primitive::FloatAdd, Ty::Float),
                Operator::Subtract => (Primitive::FloatSubtract, Ty::Float),
                Operator::Multiply => (Primitive::FloatMultiply, Ty::Float),
                Operator::Less | Operator::Equal => (Primitive::FloatLess, Ty::Bool),
            }),
            _ => None,
        };
        let Some((operation, ty)) = choice else {
            return Err(Error::Operands {
                symbol: self.symbol.clone(),
                found: format!("{}, {}", self.env.show(&left), self.env.show(&right)),
            });
        };
        let core = core::Expr::Primitive {
            operation,
            args: vec![core_left, core_right],
        };
        Ok((core, ty))
    }
}

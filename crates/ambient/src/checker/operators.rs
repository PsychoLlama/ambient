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

    /// An operator applied to operands: the first operation it can mean
    /// that takes operands of their types.
    pub(super) fn operation(
        &mut self,
        operator: Operator,
        operands: &[Expr<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        let (args, types) = self.infer_all(operands)?;
        let meant = meanings(operator).iter().find_map(|operation| {
            let ty = self.env.operation(*operation, &types)?;
            Some((*operation, ty))
        });
        let Some((operation, ty)) = meant else {
            let found: Vec<String> = types.iter().map(|ty| self.env.show(ty)).collect();
            return Err(Error::Operands {
                symbol: self.symbol.clone(),
                found: found.join(", "),
            });
        };
        Ok((core::Expr::Primitive { operation, args }, ty))
    }
}

/// The operations an operator can mean, in the order they are tried.
fn meanings(operator: Operator) -> &'static [Primitive] {
    use Primitive as P;
    match operator {
        Operator::Add => &[P::IntAdd, P::FloatAdd],
        Operator::Subtract => &[P::IntSubtract, P::FloatSubtract],
        Operator::Multiply => &[P::IntMultiply, P::FloatMultiply],
        Operator::Divide => &[P::IntDivide, P::FloatDivide],
        Operator::Remainder => &[P::IntRemainder],
        Operator::Less => &[P::IntLess, P::FloatLess, P::StringLess],
        Operator::Equal => &[P::Equal],
        Operator::Length => &[P::StringLength, P::ListLength],
        Operator::Get => &[P::ListGet],
        Operator::Push => &[P::ListPush],
        Operator::Join => &[P::StringJoin, P::ListJoin],
        Operator::Slice => &[P::StringSlice, P::ListSlice],
    }
}

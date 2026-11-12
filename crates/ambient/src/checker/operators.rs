//! Field access and operators.

use super::Checker;
use crate::core::{self, Primitive};
use crate::error::Error;
use crate::surface::{Expr, Operator, Reference};
use crate::types::{Ty, field};

impl Checker<'_> {
    /// The type of field `name` of a value of type `ty`, if every such
    /// value has one.
    pub(super) fn field(&self, ty: &Ty, name: &str) -> Option<Ty> {
        match self.env.unfold(ty) {
            Ty::Never => Some(Ty::Never),
            Ty::Named(Reference::Parameter(position), _) => {
                self.field(self.env.bounds.get(position)?, name)
            }
            Ty::Record(fields) => field(&fields, name).cloned(),
            Ty::Union(members) => {
                let fields: Option<Vec<Ty>> = members.iter().map(|m| self.field(m, name)).collect();
                fields.map(|fields| self.env.union(fields))
            }
            _ => None,
        }
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

//! Types at run time: testing a value against one, and what a cell
//! keeps of the type it was made at. See `spec/backends.md`.

use super::{Lowering, Unsupported};
use crate::core;
use crate::lir::{Constant, Expr, Op, Value};

impl Lowering {
    /// The type a cell made at `ty` keeps, as a constant. Nothing at run
    /// time knows a type parameter, so a cell made at one keeps none, and passes no cast to a cell type.
    pub(super) fn made_at(&mut self, ty: &core::Type) -> Option<usize> {
        testable(ty)
            .is_ok()
            .then(|| self.constant(Constant::Type(ty.clone())))
    }

    /// An expression that is true when `ty` describes `value`.
    pub(super) fn is(&mut self, value: Value, ty: &core::Type) -> Result<Expr, Unsupported> {
        testable(ty)?;
        let ty = self.constant(Constant::Type(ty.clone()));
        Ok(Expr::Op(Op::Is { value, ty }))
    }
}

/// Whether a value can be tested against `ty` at run time.
fn testable(ty: &core::Type) -> Result<(), Unsupported> {
    if mentions(ty, &|ty| matches!(ty, core::Type::Parameter(_))) {
        return Err(Unsupported("tests against a type parameter"));
    }
    Ok(())
}

/// Whether `found` holds of `ty` or of any type inside it.
fn mentions(ty: &core::Type, found: &impl Fn(&core::Type) -> bool) -> bool {
    use core::Type;
    found(ty)
        || match ty {
            Type::Record(fields) => fields.iter().any(|(_, ty)| mentions(ty, found)),
            Type::Tuple(members) | Type::Union(members) | Type::Reference(_, members) => {
                members.iter().any(|ty| mentions(ty, found))
            }
            Type::List(member) | Type::Cell(member) => mentions(member, found),
            Type::Function { params, ret, .. } => {
                params.iter().any(|ty| mentions(ty, found)) || mentions(ret, found)
            }
            _ => false,
        }
}

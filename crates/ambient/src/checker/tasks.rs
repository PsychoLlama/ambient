//! Tasks and cells. See `spec/tasks.md` and `spec/cells.md`.

use super::Checker;
use crate::core::{self, Primitive};
use crate::error::Error;
use crate::surface::{CellOp, Expr, Reference};
use crate::types::Ty;

impl Checker<'_> {
    /// Each branch runs as a task. A branch is a function that takes
    /// nothing, so it captures what it uses, bindings included, where the
    /// expression is written.
    pub(super) fn all(&mut self, branches: &[Expr<Reference>]) -> Result<(core::Expr, Ty), Error> {
        let mut thunks = Vec::new();
        let mut results = Vec::new();
        for branch in branches {
            let (thunk, ty) = self.lambda(&[], &[], branch, None)?;
            thunks.push(thunk);
            results.push(match ty {
                Ty::Function { ret, .. } => *ret,
                _ => Ty::Never,
            });
        }
        Ok((core::Expr::All(thunks), Ty::Tuple(results)))
    }

    pub(super) fn cell(
        &mut self,
        operation: CellOp,
        ty: Option<&Ty>,
        args: &[Expr<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        let primitive = |operation, args| core::Expr::Primitive { operation, args };
        if let (CellOp::New, Some(ty), [value]) = (operation, ty, args) {
            self.well_formed(ty)?;
            let held = self.env.normalize(ty);
            let value = self.check(value, &held)?;
            let cell = Ty::Cell(Box::new(held));
            return Ok((primitive(Primitive::CellNew, vec![value]), cell));
        }
        // Every other operation takes a cell, and then values it could
        // hold.
        let Some((cell, values)) = args.split_first() else {
            return Ok((core::Expr::Null, Ty::Never));
        };
        let (cell, found) = self.infer(cell)?;
        let Ty::Cell(held) = self.env.unfold(&found) else {
            let expected = Ty::Cell(Box::new(Ty::Unknown));
            return Err(self.mismatch(&expected, &found));
        };
        let mut core_args = vec![cell];
        for value in values {
            core_args.push(self.check(value, &held)?);
        }
        let (operation, ty) = match operation {
            CellOp::New | CellOp::Read => (Primitive::CellRead, *held),
            CellOp::Replace => (Primitive::CellReplace, Ty::Bool),
            CellOp::Wait => (Primitive::CellWait, Ty::Null),
        };
        Ok((primitive(operation, core_args), ty))
    }
}

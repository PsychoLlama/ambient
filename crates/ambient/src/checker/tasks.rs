//! Tasks, cells, and cleanups. See `spec/tasks.md`, `spec/cells.md`, and
//! `spec/cleanup.md`.

use super::Checker;
use crate::core::{self, Primitive};
use crate::error::Error;
use crate::surface::{CellOp, Expr, Reference};
use crate::types::Ty;

impl Checker<'_> {
    /// The branches of an all or a first, and what each gives. A branch
    /// runs as a task, and is a function that takes
    /// nothing, so it captures what it uses, bindings included, where the
    /// expression is written.
    pub(super) fn tasks(
        &mut self,
        branches: &[Expr<Reference>],
    ) -> Result<(Vec<core::Expr>, Vec<Ty>), Error> {
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
        Ok((thunks, results))
    }

    /// A cleanup is a function that takes nothing, made before the body
    /// runs: it has what is bound where it is registered, whatever the
    /// body's exit goes through. What it gives is dropped.
    pub(super) fn cleanup(
        &mut self,
        cleanup: &Expr<Reference>,
        body: &Expr<Reference>,
    ) -> Result<(core::Expr, Ty), Error> {
        let (cleanup, _) = self.lambda(&[], &[], cleanup, None)?;
        let (body, ty) = self.infer(body)?;
        let node = core::Expr::Cleanup {
            cleanup: Box::new(cleanup),
            body: Box::new(body),
        };
        Ok((node, ty))
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

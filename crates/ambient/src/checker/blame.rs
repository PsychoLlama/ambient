//! Holding an expression to a type, and saying where an error is.
//!
//! An error is at the innermost expression it passed through, noted by
//! address: the resolved tree stays where it is while it is checked.

use super::Checker;
use crate::core;
use crate::error::Error;
use crate::surface::{Expr, Reference};
use crate::types::Ty;

impl Checker<'_> {
    /// Checks that `expr` has a subtype of `expected`.
    pub(super) fn check(
        &mut self,
        expr: &Expr<Reference>,
        expected: &Ty,
    ) -> Result<core::Expr, Error> {
        let checked = self.entered(|checker| checker.check_node(expr, expected));
        self.blame(expr, checked)
    }

    /// Does `inside` one level further into the tree, which has room
    /// for a tree as deep as one may be, and no deeper.
    pub(super) fn entered<T>(
        &mut self,
        inside: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if self.nesting == core::DEPTH {
            return Err(Error::Nesting(self.symbol.clone()));
        }
        self.nesting += 1;
        let result = inside(self);
        self.nesting -= 1;
        result
    }

    /// Notes `expr` as where an error is, unless an expression inside
    /// it already was.
    pub(super) fn blame<T>(
        &mut self,
        expr: &Expr<Reference>,
        result: Result<T, Error>,
    ) -> Result<T, Error> {
        if result.is_err() && self.blamed.is_none() {
            self.blamed = Some(std::ptr::from_ref(expr));
        }
        result
    }

    /// Checks something the checker wrote itself, which is in no tree:
    /// an error in it is at whatever it was written for.
    pub(super) fn own<T>(
        &mut self,
        check: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let before = self.blamed;
        let result = check(self);
        if result.is_err() {
            self.blamed = before;
        }
        result
    }

    /// Where in `body` the first error was: a node's place in the order
    /// [`Expr::children`] walks them.
    pub(super) fn blamed_in(&self, body: &Expr<Reference>) -> Option<usize> {
        body.index_of(self.blamed?)
    }

    fn check_node(&mut self, expr: &Expr<Reference>, expected: &Ty) -> Result<core::Expr, Error> {
        // What a binding or a match gives is what its last part gives.
        // That part is held to what is expected, so an error is at it
        // and not at the whole.
        match expr {
            Expr::Let { value, body, .. } => {
                let (value, ty) = self.infer(value)?;
                self.bind_local(ty);
                let body = self.check(body, expected)?;
                return Ok(core::Expr::Let {
                    value: Box::new(value),
                    body: Box::new(body),
                });
            }
            Expr::Match { subject, arms } => {
                return Ok(self.matching(subject, arms, Some(expected))?.0);
            }
            Expr::Using { value, body, .. } => {
                return Ok(self.using(value, body, Some(expected))?.0);
            }
            _ => {}
        }
        let (core, found) = self.infer_as(expr, expected)?;
        let (core, found) = self.bind_row(core, found, expected)?;
        if self.env.subtype(&found, expected) {
            Ok(core)
        } else {
            Err(self.mismatch(expected, &found))
        }
    }
}

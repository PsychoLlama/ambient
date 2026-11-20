//! Holding an expression to a type, and saying where an error is.
//!
//! An error is at the innermost expression it passed through, noted by
//! address: the resolved tree stays where it is while it is checked.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::Checker;
use crate::core;
use crate::error::Error;
use crate::surface::{Expr, Reference};
use crate::symbol::Symbol;
use crate::types::Ty;

/// For a function, and for each lambda in it by its path: the node of
/// the surface body each expression of its core body was checked from,
/// by [position](core::Expr::positions).
pub type Origins = BTreeMap<Vec<usize>, Vec<u32>>;

/// What checking a function notes beside its tree, about the nodes of
/// its surface body: each by its place in the order [`Expr::children`]
/// walks them. None of it reaches a hash.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Noted {
    /// The node the first error is at, if it is at one.
    pub at: Option<usize>,
    /// What each expression that was made came from.
    pub origins: Origins,
    /// The definitions that code a comptime call gave names: what the
    /// check read that the body does not say.
    pub reached: Vec<Symbol>,
    /// The code a comptime call gave that the first error is in,
    /// written out with the node it is at marked. The error is then at
    /// the call, which is all of that code the text has.
    pub given: Option<String>,
}

impl Checker<'_> {
    /// Checks that `expr` has a subtype of `expected`.
    pub(super) fn check(
        &mut self,
        expr: &Expr<Reference>,
        expected: &Ty,
    ) -> Result<core::Expr, Error> {
        let checked = self.entered(|checker| checker.check_node(expr, expected));
        let core = self.blame(expr, checked)?;
        Ok(self.marked(expr, core))
    }

    /// Numbers every node of `body`, the tree about to be checked, so
    /// that what is made from one can say so.
    pub(super) fn place(&mut self, body: &Expr<Reference>) {
        let mut pending = vec![body];
        while let Some(expr) = pending.pop() {
            let number = u32::try_from(self.places.len()).unwrap_or(u32::MAX);
            self.places.insert(std::ptr::from_ref(expr), number);
            let from = pending.len();
            expr.children(&mut |child| pending.push(child));
            // The first child is to be numbered first.
            pending[from..].reverse();
        }
    }

    /// `core`, marked as made from `expr`, if `expr` is a node of the
    /// body: what the checker wrote itself is from whatever holds it.
    pub(super) fn marked(&self, expr: &Expr<Reference>, core: core::Expr) -> core::Expr {
        match self.places.get(&std::ptr::from_ref(expr)) {
            Some(node) => core::Expr::At(*node, Box::new(core)),
            None => core,
        }
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

    /// A binding of `value` around `body`, which is held to `expected`
    /// if anything is. What follows a value of type `never` is never
    /// reached: it is held to nothing, and the binding is `never` too.
    /// So a block may end in a call that does not return.
    pub(super) fn binding(
        &mut self,
        value: &Expr<Reference>,
        body: &Expr<Reference>,
        expected: Option<&Ty>,
    ) -> Result<(core::Expr, Ty), Error> {
        let (value, ty) = self.infer(value)?;
        let ends = self.env.normalize(&ty) == Ty::Never;
        self.bind_local(ty);
        let (body, ty) = match expected.filter(|_| !ends) {
            Some(expected) => (self.check(body, expected)?, expected.clone()),
            None => self.infer(body)?,
        };
        let core = core::Expr::Let {
            value: Box::new(value),
            body: Box::new(body),
        };
        Ok((core, if ends { Ty::Never } else { ty }))
    }

    fn check_node(&mut self, expr: &Expr<Reference>, expected: &Ty) -> Result<core::Expr, Error> {
        // What a binding or a match gives is what its last part gives.
        // That part is held to what is expected, so an error is at it
        // and not at the whole.
        match expr {
            Expr::Let { value, body, .. } => {
                return Ok(self.binding(value, body, Some(expected))?.0);
            }
            Expr::Match { subject, arms } => {
                return Ok(self.matching(subject, arms, Some(expected))?.0);
            }
            Expr::Using { value, body, .. } => {
                return Ok(self.using(value, body, Some(expected))?.0);
            }
            Expr::Comptime(call) if self.gives_within(call, expected) => {
                return Ok(self.comptime(call)?.0);
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

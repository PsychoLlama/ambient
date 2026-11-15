//! Where in a definition an error is: which node of its body, counted
//! in the order [`crate::surface::Expr::children`] walks them. A reader
//! that kept where it found each node (`syntax::Placed`) turns that into
//! a line.

use super::Engine;
use crate::error::Error;
use crate::symbol::Symbol;

impl Engine<'_> {
    /// The node of its definition's body that `error` is at, if it is
    /// at one: where checking found it, or where resolving did.
    #[must_use]
    pub fn blame(&self, error: &Error) -> Option<usize> {
        let symbol = error.symbol()?;
        let checked = self.blamed.borrow().get(symbol).copied();
        checked.or_else(|| self.unresolved_at(symbol))
    }

    /// The node of its definition's surface body that the expression
    /// at `position` of a function's core body was checked from: what
    /// turns a frame of a trace into a place in the text. `function`
    /// may be a lambda, whose nodes are its named definition's.
    #[must_use]
    pub fn origin(&self, function: &Symbol, position: usize) -> Option<usize> {
        let named = function.lambda(&[]);
        self.check(&named).ok()?;
        let origins = self.origins.borrow();
        let node = origins.get(&named)?.get(&function.lambda)?.get(position)?;
        usize::try_from(*node).ok()
    }

    /// The node each expression of a function's core body was checked
    /// from, by position: every answer [`Self::origin`] has for it.
    #[must_use]
    pub fn origins(&self, function: &Symbol) -> Vec<Option<usize>> {
        let named = function.lambda(&[]);
        if self.check(&named).is_err() {
            return Vec::new();
        }
        let origins = self.origins.borrow();
        let nodes = origins
            .get(&named)
            .and_then(|lambdas| lambdas.get(&function.lambda));
        let node = |node: &u32| usize::try_from(*node).ok();
        nodes.map_or_else(Vec::new, |nodes| nodes.iter().map(node).collect())
    }
}

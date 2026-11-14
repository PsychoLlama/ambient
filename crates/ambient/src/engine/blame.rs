//! Where in a definition an error is: which node of its body, counted
//! in the order [`crate::surface::Expr::children`] walks them. A reader
//! that kept where it found each node (`syntax::Placed`) turns that into
//! a line.

use super::Engine;
use crate::error::Error;

impl Engine<'_> {
    /// The node of its definition's body that `error` is at, if it is
    /// at one: where checking found it, or where resolving did.
    #[must_use]
    pub fn blame(&self, error: &Error) -> Option<usize> {
        let symbol = error.symbol()?;
        let checked = self.blamed.borrow().get(symbol).copied();
        checked.or_else(|| self.unresolved_at(symbol))
    }
}

//! Verify: whether a checked definition is one the core checker accepts.
//! See `spec/exchange.md`.

use super::Engine;
use crate::error::Error;
use crate::symbol::Symbol;
use crate::verify::verify;

impl Engine<'_> {
    /// Verifies a definition's core tree, as a receiver of it would.
    ///
    /// Checking a definition never needs this: it is how the two
    /// checkers are held to each other.
    ///
    /// # Errors
    ///
    /// Fails if the definition does not check, or if the core checker
    /// refuses what checking made.
    pub fn verify(&self, symbol: &Symbol) -> Result<(), Error> {
        let open = self.check(symbol)?;
        let root = self.root_of(&symbol.owner()).is_some();
        verify(&open, root, &|slot| self.check(slot).ok()).map_err(|problem| Error::Invalid {
            symbol: symbol.clone(),
            problem,
        })
    }
}

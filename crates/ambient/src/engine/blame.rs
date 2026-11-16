//! Where in a definition an error is: which node of its body, counted
//! in the order [`crate::surface::Expr::children`] walks them. A reader
//! that kept where it found each node (`syntax::Placed`) turns that into
//! a line.

use super::Engine;
use crate::error::Error;
use crate::surface::{DefinitionKind, GENERATOR, shown};
use crate::symbol::Symbol;

impl Engine<'_> {
    /// The node of its definition's body that `error` is at, if it is
    /// at one: where checking found it, or where resolving did.
    #[must_use]
    pub fn blame(&self, error: &Error) -> Option<usize> {
        let symbol = error.symbol()?;
        // What a generated module has is nowhere in the text.
        if self.made_in_module(symbol) {
            return None;
        }
        let checked = self.blamed.borrow().get(symbol).copied();
        checked.or_else(|| self.unresolved_at(symbol))
    }

    /// Whether a definition is one a generated module's call gave.
    fn made_in_module(&self, symbol: &Symbol) -> bool {
        symbol.name != GENERATOR
            && self
                .generates(&symbol.module)
                .is_ok_and(|generates| generates)
    }

    /// Whether a function is whatever a comptime call gave: a generated
    /// function, or one of a generated module. No node of one is in
    /// the text, so a frame of a trace in one is at the call.
    #[must_use]
    pub fn is_given(&self, symbol: &Symbol) -> bool {
        let named = symbol.lambda(&[]);
        let generated = self
            .resolve(&named)
            .is_ok_and(|resolved| matches!(resolved.definition.kind, DefinitionKind::Generated(_)));
        generated || self.made_in_module(&named)
    }

    /// The code a comptime call gave that `error` is in, written out
    /// around the node it is at: a function that was generated, or an
    /// expression that was spliced. Nothing, for an error in what was
    /// written.
    #[must_use]
    pub fn shown(&self, error: &Error) -> Option<String> {
        let symbol = error.symbol()?;
        if let Some(spliced) = self.spliced.borrow().get(symbol) {
            return Some(spliced.clone());
        }
        let node = match self.given_at.borrow().get(symbol) {
            Some(node) => Some(*node),
            None if self.made_in_module(symbol) => self.blamed.borrow().get(symbol).copied(),
            None => return None,
        };
        let resolved = self.resolve(symbol).ok()?;
        let function = self.function_of(symbol, &resolved.definition).ok()??;
        let shown = shown::function(&symbol.name, &symbol.module, &function, node);
        Some(shown.excerpt())
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

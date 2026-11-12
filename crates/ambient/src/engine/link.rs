//! Link: everything an entry reaches, compiled, as the parts of a program.
//! See `spec/build.md` and `spec/targets.md`.

use std::collections::HashMap;
use std::sync::Arc;

use super::{Artifact, Backend, Engine};
use crate::core::Tree;
use crate::error::Error;
use crate::hash::Hash;
use crate::symbol::Symbol;

/// One compiled function of a program.
#[derive(Debug)]
pub struct Part {
    pub hash: Hash,
    /// How many parameters it takes.
    pub arity: usize,
    pub code: Arc<Artifact>,
    /// The part each slot refers to. `None` for a slot that is not a
    /// function.
    pub slots: Vec<Option<usize>>,
}

/// A program: exactly the functions its entry reaches, each once.
#[derive(Debug)]
pub struct Parts {
    pub parts: Vec<Part>,
    /// The entry's position in `parts`.
    pub entry: usize,
}

impl Engine<'_> {
    /// Compiles `entry` and everything it reaches.
    ///
    /// Definitions with equal hashes are one part, whatever their symbols.
    ///
    /// # Errors
    ///
    /// Fails if anything reached does not check or does not compile.
    pub fn link(&self, entry: &Symbol, backend: &dyn Backend) -> Result<Parts, Error> {
        let mut indices: HashMap<Hash, usize> = HashMap::new();
        let mut symbols: Vec<Symbol> = Vec::new();
        let mut pending = vec![entry.clone()];
        while let Some(symbol) = pending.pop() {
            let open = self.check(&symbol)?;
            if !matches!(open.tree, Tree::Function(_)) {
                continue;
            }
            let hash = self.hash(&symbol)?;
            if indices.contains_key(&hash) {
                continue;
            }
            indices.insert(hash, symbols.len());
            symbols.push(symbol);
            pending.extend(open.slots.iter().cloned());
        }

        let mut parts = Vec::new();
        for symbol in &symbols {
            let open = self.check(symbol)?;
            let arity = match &open.tree {
                Tree::Function(function) => function.params.len(),
                Tree::Type { .. } => 0,
            };
            let mut slots = Vec::new();
            for slot in &open.slots {
                let is_function = matches!(self.check(slot)?.tree, Tree::Function(_));
                slots.push(if is_function {
                    indices.get(&self.hash(slot)?).copied()
                } else {
                    None
                });
            }
            parts.push(Part {
                hash: self.hash(symbol)?,
                arity,
                code: self.compile(symbol, backend)?,
                slots,
            });
        }
        let entry = indices.get(&self.hash(entry)?).copied().unwrap_or(0);
        Ok(Parts { parts, entry })
    }
}

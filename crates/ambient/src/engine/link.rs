//! Link: everything an entry reaches, as the parts of a program. See
//! `spec/build.md` and `spec/targets.md`.

use std::collections::HashMap;
use std::sync::Arc;

use super::{Artifact, Backend, Engine};
use crate::core::{self, Tree};
use crate::error::Error;
use crate::hash::Hash;
use crate::lir::Constant;
use crate::lower::lower;
use crate::symbol::Symbol;

/// One definition of a program.
#[derive(Debug)]
pub struct Part {
    pub hash: Hash,
    /// The part each slot refers to.
    pub slots: Vec<usize>,
    pub body: Body,
}

#[derive(Debug)]
pub enum Body {
    Function {
        /// How many parameters it takes.
        arity: usize,
        code: Arc<Artifact>,
        /// What the function needs made when the program is loaded.
        constants: Vec<Constant>,
    },
    /// A recursive type definition, which casts test values against.
    Type(core::Type),
}

/// A program: exactly the definitions its entry reaches, each once.
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
            let hash = self.hash(&symbol)?;
            if indices.contains_key(&hash) {
                continue;
            }
            indices.insert(hash, symbols.len());
            pending.extend(self.check(&symbol)?.slots.iter().cloned());
            symbols.push(symbol);
        }

        let mut parts = Vec::new();
        for symbol in &symbols {
            let open = self.check(symbol)?;
            let mut slots = Vec::new();
            for slot in &open.slots {
                slots.push(indices.get(&self.hash(slot)?).copied().unwrap_or(0));
            }
            let body = match &open.tree {
                Tree::Function(function) => Body::Function {
                    arity: function.params.len(),
                    code: self.compile(symbol, backend)?,
                    // Lowering is cheap and has just succeeded inside
                    // `compile`, so its constants are read from it again
                    // rather than kept beside the code.
                    constants: lower(&open)
                        .map(|function| function.constants)
                        .unwrap_or_default(),
                },
                Tree::Type { ty, .. } => Body::Type(ty.clone()),
            };
            parts.push(Part {
                hash: self.hash(symbol)?,
                slots,
                body,
            });
        }
        let entry = indices.get(&self.hash(entry)?).copied().unwrap_or(0);
        Ok(Parts { parts, entry })
    }
}

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
    /// If the part is an extern: its position in [`Parts::natives`].
    pub native: Option<usize>,
}

/// An extern a program reaches: what a host finds native code by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Native {
    pub package: String,
    pub tag: String,
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
    /// An ability or a target. Nothing is laid out for one: an ability
    /// is reached by a key made from its hash, and a target not at all.
    Inert,
}

/// A program: exactly the definitions its entry reaches, each once.
#[derive(Debug)]
pub struct Parts {
    pub parts: Vec<Part>,
    /// The entry's position in `parts`.
    pub entry: usize,
    /// The externs among `parts`, which a host must supply before the
    /// program runs.
    pub natives: Vec<Native>,
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
        // TODO: an entry runs under a target, which provides its row; until
        // there are targets, an entry has nothing to take.
        if let Tree::Function(function) = &self.check(entry)?.tree
            && !function.row.is_empty()
        {
            return Err(Error::EntryAbilities(entry.clone()));
        }
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
        let mut natives = Vec::new();
        for symbol in &symbols {
            let open = self.check(symbol)?;
            let mut slots = Vec::new();
            for slot in &open.slots {
                slots.push(indices.get(&self.hash(slot)?).copied().unwrap_or(0));
            }
            let body = match &open.tree {
                Tree::Function(function) => Body::Function {
                    // The bindings a function is handed come first.
                    arity: function.params.len() + 1,
                    code: self.compile(symbol, backend)?,
                    // Lowering is cheap and has just succeeded inside
                    // `compile`, so its constants are read from it again
                    // rather than kept beside the code.
                    constants: lower(&open)
                        .map(|function| function.constants)
                        .unwrap_or_default(),
                },
                Tree::Extern { params, .. } => Body::Function {
                    arity: params.len() + 1,
                    code: self.compile(symbol, backend)?,
                    constants: lower(&open)
                        .map(|function| function.constants)
                        .unwrap_or_default(),
                },
                Tree::Type { ty, .. } => Body::Type(ty.clone()),
                Tree::Ability { .. } | Tree::Target { .. } => Body::Inert,
            };
            let native = match &open.tree {
                Tree::Extern { package, tag, .. } => {
                    natives.push(Native {
                        package: package.clone(),
                        tag: tag.clone(),
                    });
                    Some(natives.len() - 1)
                }
                _ => None,
            };
            parts.push(Part {
                hash: self.hash(symbol)?,
                slots,
                body,
                native,
            });
        }
        let entry = indices.get(&self.hash(entry)?).copied().unwrap_or(0);
        Ok(Parts {
            parts,
            entry,
            natives,
        })
    }
}

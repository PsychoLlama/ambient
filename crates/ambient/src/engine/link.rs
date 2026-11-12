//! Link: everything an entry reaches, as the parts of a program. See
//! `spec/build.md` and `spec/targets.md`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{Artifact, Backend, Engine};
use crate::core::{self, Tree};
use crate::error::Error;
use crate::hash::Hash;
use crate::lir::Constant;
use crate::lower::lower;
use crate::symbol::Symbol;

/// One definition of a program.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    pub hash: Hash,
    /// The part each slot refers to.
    pub slots: Vec<usize>,
    pub body: Body,
    /// If the part is an extern: its position in [`Parts::natives`].
    pub native: Option<usize>,
}

/// An extern a program reaches: what a host finds native code by.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Native {
    pub package: String,
    pub tag: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Body {
    Function {
        /// How many parameters it takes.
        arity: usize,
        code: Arc<Artifact>,
        /// Its code as what runs inside a budget calls it, if the
        /// program enters one.
        metered: Option<Arc<Artifact>>,
        /// What the function needs made when the program is loaded.
        constants: Vec<Constant>,
        /// Its type, which its definition carries the name of, unless it
        /// has type parameters. Its slots are the definition's.
        signature: Option<core::Type>,
    },
    /// A recursive type definition, which casts test values against.
    Type(core::Type),
    /// An ability or a target. Nothing is laid out for one: an ability
    /// is reached by a key made from its hash, and a target not at all.
    Inert,
}

impl Parts {
    /// The code of the program: for each part, its code, and then its
    /// metered code. A part that has neither, or no metered code, has
    /// `None` there. With each is how many parameters it takes.
    #[must_use]
    pub fn codes(&self) -> Vec<Option<(usize, &Artifact)>> {
        let mut codes = Vec::new();
        for part in &self.parts {
            let (plain, metered) = match &part.body {
                Body::Function {
                    arity,
                    code,
                    metered,
                    ..
                } => (
                    Some((*arity, &**code)),
                    metered.as_deref().map(|code| (*arity, code)),
                ),
                Body::Type(_) | Body::Inert => (None, None),
            };
            codes.extend([plain, metered]);
        }
        codes
    }
}

/// A program: exactly the definitions its entry reaches, each once.
/// [`Parts::encode`] is its form for a host that has no compiler.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parts {
    pub parts: Vec<Part>,
    /// The entry's position in `parts`.
    pub entry: usize,
    /// The externs among `parts`, which a host must supply before the
    /// program runs.
    pub natives: Vec<Native>,
}

impl Engine<'_> {
    /// The definitions among `symbols`, a whole program, that can run
    /// inside a budget, by hash: those get a metered form.
    ///
    /// What runs inside a budget is what its body names, and any
    /// function value, since one may have been made anywhere and is
    /// called wherever it arrives; and then whatever those name. If
    /// nothing enters a budget, nothing does.
    fn metered(&self, symbols: &[Symbol]) -> Result<HashSet<Hash>, Error> {
        let mut pending = Vec::new();
        let mut budgets = false;
        for symbol in symbols {
            let open = self.check(symbol)?;
            let Tree::Function(function) = &open.tree else {
                continue;
            };
            let named = |slot: &core::Slot| open.slots.get(slot.0).cloned();
            pending.extend(function.body.values().iter().filter_map(named));
            if let Some(inside) = function.body.budgeted() {
                budgets = true;
                pending.extend(inside.iter().filter_map(named));
            }
        }
        let mut found = HashSet::new();
        while let Some(symbol) = pending.pop().filter(|_| budgets) {
            if found.insert(self.hash(&symbol)?) {
                pending.extend(self.check(&symbol)?.slots.iter().cloned());
            }
        }
        Ok(found)
    }

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
        self.parts(entry, backend, false)
    }

    /// Compiles `function` and everything it reaches, to be loaded into
    /// a program that is running. Where it will be called from is not
    /// known, so every function gets a metered form, and it may take
    /// abilities: whatever calls it provides them.
    ///
    /// # Errors
    ///
    /// Fails if anything reached does not check or does not compile.
    pub fn link_to_load(&self, function: &Symbol, backend: &dyn Backend) -> Result<Parts, Error> {
        self.parts(function, backend, true)
    }

    fn parts(&self, entry: &Symbol, backend: &dyn Backend, loaded: bool) -> Result<Parts, Error> {
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

        let metered = if loaded {
            let hashes = symbols.iter().map(|symbol| self.hash(symbol));
            hashes.collect::<Result<_, _>>()?
        } else {
            self.metered(&symbols)?
        };

        let mut parts = Vec::new();
        let mut natives = Vec::new();
        for symbol in &symbols {
            let open = self.check(symbol)?;
            let mut slots = Vec::new();
            for slot in &open.slots {
                slots.push(indices.get(&self.hash(slot)?).copied().unwrap_or(0));
            }
            let metered = metered.contains(&self.hash(symbol)?);
            // The bindings a function is handed come first.
            let function = |generic: bool, params: &[core::Type], ret, row: &[core::Slot]| {
                let signature = core::Type::Function {
                    params: params.to_vec(),
                    ret: Box::new(Clone::clone(ret)),
                    row: row.to_vec(),
                };
                Ok::<Body, Error>(Body::Function {
                    arity: params.len() + 1,
                    signature: (!generic).then_some(signature),
                    code: self.compile_as(symbol, backend, false)?,
                    metered: if metered {
                        Some(self.compile_as(symbol, backend, true)?)
                    } else {
                        None
                    },
                    // Lowering is cheap and has just succeeded inside
                    // `compile`, so its constants are read from it again
                    // rather than kept beside the code. Those of the
                    // metered form begin with those of the plain one.
                    constants: lower(&open, &self.form(symbol, metered)?)
                        .map(|function| function.constants)
                        .unwrap_or_default(),
                })
            };
            let body = match &open.tree {
                Tree::Function(tree) => {
                    let generic = !tree.type_params.is_empty();
                    function(generic, &tree.params, &tree.ret, &tree.row)?
                }
                Tree::Extern {
                    params, ret, row, ..
                } => function(false, params, ret, row)?,
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

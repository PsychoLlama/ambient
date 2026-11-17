//! Link: everything an entry reaches, as the parts of a program. See
//! `spec/build.md` and `spec/targets.md`.

use std::collections::HashMap;

use super::{Backend, Body, Engine, Native, Part, Parts};
use crate::core::{self, Tree};
use crate::error::Error;
use crate::hash::Hash;
use crate::lower::{deep, lower};
use crate::symbol::Symbol;

/// What parts are linked as.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A program nothing is loaded into.
    Closed,
    /// A program code can be loaded into.
    Open,
    /// Code to load into a program.
    Loaded,
}

/// What the definition of a function of type `function` says of its
/// type: the type it carries the name of, and the type it carries a
/// descriptor of. `bounds` are those of its type parameters, the first
/// `outer` of which are its enclosers'.
fn says(
    function: core::Type,
    bounds: &[core::Type],
    outer: usize,
    handed: bool,
) -> (Option<core::Type>, Option<core::Type>) {
    if bounds.is_empty() {
        return (Some(function), None);
    }
    // What it is at the type arguments a value of it holds.
    let described = handed.then(|| function.clone());
    // A generic function is of a generic function type. One written
    // inside another is of a type that depends on the type arguments
    // of what it is written in, which has no name.
    let named = (outer == 0).then(|| core::Type::Generic {
        bounds: bounds.iter().map(|bound| bound.generalized(0)).collect(),
        function: Box::new(function.generalized(0)),
    });
    (named, described)
}

impl Engine<'_> {
    /// Compiles `entry` and everything it reaches, as a program that
    /// nothing is loaded into.
    ///
    /// The whole program is known, so it pays only for what it does:
    /// if nothing in it can suspend a task, no function is compiled to
    /// suspend. See "Floor" in `spec/linking.md`.
    ///
    /// Definitions with equal hashes are one part, whatever their symbols.
    ///
    /// # Errors
    ///
    /// Fails if anything reached does not check or does not compile.
    pub fn link(&self, entry: &Symbol, backend: &dyn Backend) -> Result<Parts, Error> {
        self.program_parts(entry, backend, Shape::Closed)
    }

    /// Compiles `entry` and everything it reaches, as a program that
    /// code can be loaded into: see [`Engine::link_to_load`]. What is
    /// loaded may suspend a task in the middle of any call, so every
    /// function is compiled to.
    ///
    /// # Errors
    ///
    /// Fails as [`Engine::link`] does.
    pub fn link_open(&self, entry: &Symbol, backend: &dyn Backend) -> Result<Parts, Error> {
        self.program_parts(entry, backend, Shape::Open)
    }

    fn program_parts(
        &self,
        entry: &Symbol,
        backend: &dyn Backend,
        shape: Shape,
    ) -> Result<Parts, Error> {
        // Nothing is bound where a host calls from. An entry that takes
        // abilities runs under a target: see [`Engine::program`].
        if let Tree::Function(function) = &self.check(entry)?.tree
            && !function.row.is_empty()
        {
            return Err(Error::EntryAbilities(entry.clone()));
        }
        self.parts(entry, backend, shape)
    }

    /// Whether nothing among `symbols`, a whole program, can suspend a
    /// task.
    fn still(&self, symbols: &[Symbol]) -> Result<bool, Error> {
        for symbol in symbols {
            let suspends = match &self.check(symbol)?.tree {
                Tree::Function(function) => function.body.suspends(),
                Tree::Extern { suspends, .. } => *suspends,
                _ => false,
            };
            if suspends {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Compiles `function` and everything it reaches, to be loaded into
    /// a program that is running. It may take abilities: whatever calls it
    /// provides them.
    /// A host that calls it itself provides nothing, so it loads the
    /// function under a target, as [`Engine::program`] makes it.
    ///
    /// # Errors
    ///
    /// Fails if anything reached does not check or does not compile.
    pub fn link_to_load(&self, function: &Symbol, backend: &dyn Backend) -> Result<Parts, Error> {
        self.parts(function, backend, Shape::Loaded)
    }

    fn parts(&self, entry: &Symbol, backend: &dyn Backend, shape: Shape) -> Result<Parts, Error> {
        deep(|| self.reached(entry, backend, shape))
    }

    /// Everything `entry` reaches, itself first, each hash once: the
    /// symbols, and where each hash is among them.
    fn reach(&self, entry: &Symbol) -> Result<(HashMap<Hash, usize>, Vec<Symbol>), Error> {
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
        Ok((indices, symbols))
    }

    /// How many of a function's type parameters are those of what it
    /// is written in: none, unless it is a lambda.
    fn enclosing(&self, symbol: &Symbol) -> Result<usize, Error> {
        let Some((_, path)) = symbol.lambda.split_last() else {
            return Ok(0);
        };
        Ok(match &self.check(&symbol.lambda(path))?.tree {
            Tree::Function(encloser) => encloser.type_params.len(),
            _ => 0,
        })
    }

    /// What a host knows an ability by: its package and its module
    /// path and name, as it knows an extern.
    fn named(&self, ability: &Symbol) -> (String, String) {
        let package = self.sources.identity(&ability.module.package);
        (package, ability.identity())
    }

    fn reached(&self, entry: &Symbol, backend: &dyn Backend, shape: Shape) -> Result<Parts, Error> {
        let (indices, symbols) = self.reach(entry)?;

        let still = backend.stacks() || shape == Shape::Closed && self.still(&symbols)?;

        let mut parts = Vec::new();
        let mut natives = Vec::new();
        for symbol in &symbols {
            let open = self.check(symbol)?;
            let mut slots = Vec::new();
            for slot in &open.slots {
                slots.push(indices.get(&self.hash(slot)?).copied().unwrap_or(0));
            }
            // The bindings a function is handed come first.
            let function = |bounds: &[core::Type], params: &[core::Type], ret, row: &[_]| {
                let handed = self.handed(symbol)?;
                let function = core::Type::Function {
                    params: params.to_vec(),
                    ret: Box::new(Clone::clone(ret)),
                    row: row.to_vec(),
                };
                let outer = self.enclosing(symbol)?;
                let (signature, described) = says(function, bounds, outer, handed);
                Ok::<Body, Error>(Body::Function {
                    arity: params.len() + 1,
                    signature,
                    described,
                    types: if handed { bounds.len() } else { 0 },
                    code: self.compile_as(symbol, backend, still)?,
                    // Lowering is cheap and has just succeeded inside
                    // `compile`, so its constants are read from it again
                    // rather than kept beside the code.
                    constants: {
                        let form = self.form(symbol, still)?;
                        let lowered = lower(&open, &form);
                        lowered.map_or_else(|_| Vec::new(), |function| function.constants)
                    },
                })
            };
            let body = match &open.tree {
                Tree::Function(tree) => {
                    function(&tree.type_params, &tree.params, &tree.ret, &tree.row)?
                }
                Tree::Extern {
                    params, ret, row, ..
                } => function(&[], params, ret, row)?,
                Tree::Type { ty, .. } => Body::Type(ty.clone()),
                Tree::Ability { .. } | Tree::Key { .. } | Tree::Target { .. } => Body::Inert,
            };
            let native = match &open.tree {
                Tree::Extern {
                    package, path, row, ..
                } => {
                    let named = |slot: &core::Slot| self.named(&open.slots[slot.0]);
                    natives.push(Native {
                        package: package.clone(),
                        path: path.clone(),
                        row: row.iter().map(named).collect(),
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
            open: shape != Shape::Closed,
        })
    }
}

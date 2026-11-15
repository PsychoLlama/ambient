//! Which functions are handed their type arguments as they run. See
//! "Types at run time" in `spec/backends.md`.

use std::collections::HashMap;

use super::Engine;
use super::compile::Mode;
use crate::core::Tree;
use crate::error::Error;
use crate::lower::Form;
use crate::surface::{DefinitionKind, Function};
use crate::symbol::Symbol;

/// A function, as far as its type arguments go.
struct Node {
    /// Whether it needs them itself, or is known to need them.
    needs: bool,
    /// The generic functions it hands them on to.
    through: Vec<Symbol>,
}

impl Engine<'_> {
    /// Whether a function needs its type arguments as it runs: it tests
    /// a value against one, or makes a cell at one, or hands one on to a
    /// function that needs it.
    ///
    /// Functions can hand them on in a circle, so this is the least
    /// answer that holds for everything `symbol` reaches that way: a
    /// function that only calls itself needs none.
    ///
    /// # Errors
    ///
    /// Fails if a definition it reaches does not check.
    pub(super) fn handed(&self, symbol: &Symbol) -> Result<bool, Error> {
        if let Some(known) = self.handed.borrow().get(symbol) {
            return Ok(*known);
        }
        let mut nodes: HashMap<Symbol, Node> = HashMap::new();
        let mut pending = vec![symbol.clone()];
        while let Some(symbol) = pending.pop() {
            if nodes.contains_key(&symbol) {
                continue;
            }
            let known = self.handed.borrow().get(&symbol).copied();
            let node = match known {
                Some(needs) => Node {
                    needs,
                    through: Vec::new(),
                },
                None => self.uses_types(&symbol)?,
            };
            pending.extend(node.through.iter().cloned());
            nodes.insert(symbol, node);
        }
        // Whatever hands them on to one that needs them needs them.
        loop {
            let found: Vec<Symbol> = nodes
                .iter()
                .filter(|(_, node)| !node.needs)
                .filter(|(_, node)| node.through.iter().any(|next| nodes[next].needs))
                .map(|(symbol, _)| symbol.clone())
                .collect();
            if found.is_empty() {
                break;
            }
            for symbol in found {
                nodes.entry(symbol).and_modify(|node| node.needs = true);
            }
        }
        let mut handed = self.handed.borrow_mut();
        handed.extend(nodes.into_iter().map(|(symbol, node)| (symbol, node.needs)));
        Ok(handed[symbol])
    }

    fn uses_types(&self, symbol: &Symbol) -> Result<Node, Error> {
        let none = Node {
            needs: false,
            through: Vec::new(),
        };
        // What has no type parameters needs none, whatever its body, so
        // its body is not looked at: compiling what uses a definition
        // does not wait on that definition checking.
        let written = self.received(symbol).is_none() && self.root_of(symbol).is_none();
        if symbol.lambda.is_empty() && written {
            let resolved = self.resolve(symbol)?;
            let generic = |function: &Function<_>| !function.type_params.is_empty();
            if !matches!(&resolved.definition.kind, DefinitionKind::Function(f) if generic(f)) {
                return Ok(none);
            }
        }
        let open = self.check(symbol)?;
        let Tree::Function(function) = &open.tree else {
            return Ok(none);
        };
        let consults = function.body.consults();
        let through = consults.through.iter();
        Ok(Node {
            needs: consults.own,
            through: through.map(|slot| open.slots[slot.0].clone()).collect(),
        })
    }

    /// What lowering `symbol` takes besides its tree.
    pub(super) fn form(&self, symbol: &Symbol, mode: Mode) -> Result<Form, Error> {
        let open = self.check(symbol)?;
        let slots = open.slots.iter().map(|slot| self.handed(slot));
        Ok(Form {
            metered: mode.metered,
            still: mode.still,
            handed: self.handed(symbol)?,
            slots: slots.collect::<Result<_, _>>()?,
            own: open.slots.iter().position(|slot| slot == symbol),
        })
    }
}

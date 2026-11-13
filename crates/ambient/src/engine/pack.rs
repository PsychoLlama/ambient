//! Packing a definition to send, and receiving one. See
//! `spec/exchange.md`.
//!
//! A received definition enters the engine as an open core tree under a
//! symbol the engine makes up: a symbol never reaches a hash, so the
//! definition hashes, compiles, and links as it did where it came from.
//! What it skips is everything before the core tree, and the store.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::Engine;
use crate::core::{Open, Tree};
use crate::error::Error;
use crate::hash::Hash;
use crate::lower::deep;
use crate::pack::{Closed, Pack, Refused, Target};
use crate::symbol::{ModuleId, Namespace, Symbol};
use crate::verify::verify;

/// The package received definitions are named in. No workspace has one
/// of this name.
const RECEIVED: &str = "@received";

impl Engine<'_> {
    /// Every definition `roots` reach, by hash.
    fn by_hash(&self, roots: &[Symbol]) -> Result<HashMap<Hash, Symbol>, Error> {
        let mut found = HashMap::new();
        let mut seen = HashSet::new();
        let mut pending = roots.to_vec();
        while let Some(symbol) = pending.pop() {
            if !seen.insert(symbol.clone()) {
                continue;
            }
            pending.extend(self.check(&symbol)?.slots.iter().cloned());
            found.entry(self.hash(&symbol)?).or_insert(symbol);
        }
        Ok(found)
    }

    /// Packs `entry` with everything it reaches, apart from what `has`
    /// says the receiver has already.
    ///
    /// # Errors
    ///
    /// Fails if anything reached does not check.
    pub fn pack(&self, entry: &Symbol, has: &dyn Fn(&Hash) -> bool) -> Result<Pack, Error> {
        deep(|| self.packed(entry, has))
    }

    fn packed(&self, entry: &Symbol, has: &dyn Fn(&Hash) -> bool) -> Result<Pack, Error> {
        let mut positions: HashMap<Hash, usize> = HashMap::new();
        let mut symbols = Vec::new();
        let mut pending = vec![entry.clone()];
        while let Some(symbol) = pending.pop() {
            let hash = self.hash(&symbol)?;
            // The entry is sent whatever the receiver has.
            if positions.contains_key(&hash) || (has(&hash) && !symbols.is_empty()) {
                continue;
            }
            positions.insert(hash, symbols.len());
            pending.extend(self.check(&symbol)?.slots.iter().cloned());
            symbols.push(symbol);
        }
        let mut definitions = Vec::new();
        for symbol in &symbols {
            let open = self.check(symbol)?;
            let mut slots = Vec::new();
            for slot in &open.slots {
                let hash = self.hash(slot)?;
                slots.push(match positions.get(&hash) {
                    Some(position) => Target::Inside(*position),
                    None => Target::Outside(hash),
                });
            }
            definitions.push(Closed {
                tree: open.tree.clone(),
                slots,
            });
        }
        Ok(Pack {
            definitions,
            entry: 0,
        })
    }

    /// Takes in a pack, and gives the symbol its entry has here.
    ///
    /// Nothing the pack says is trusted. Each definition is checked
    /// against the headers of what it refers to, and hashes are worked
    /// out from the trees. A slot that refers outside the pack must be a
    /// definition `known` reaches.
    ///
    /// # Errors
    ///
    /// Fails if the pack refers to a definition the receiver lacks, or
    /// holds a tree checking could not have made. Nothing of it is kept
    /// then.
    pub fn receive(&self, pack: &Pack, known: &[Symbol]) -> Result<Symbol, Error> {
        deep(|| self.taken(pack, known))
    }

    fn taken(&self, pack: &Pack, known: &[Symbol]) -> Result<Symbol, Error> {
        let known = self.by_hash(known)?;
        let batch = self.received.borrow().len();
        let module = ModuleId::new(RECEIVED, "");
        let symbol = |position: usize, tree: &Tree| {
            let namespace = match tree {
                Tree::Type { .. } => Namespace::Type,
                Tree::Ability { .. } => Namespace::Ability,
                Tree::Key { .. } => Namespace::Key,
                _ => Namespace::Value,
            };
            module.symbol(namespace, &format!("{batch}.{position}"))
        };
        let symbols: Vec<Symbol> = (0..)
            .zip(&pack.definitions)
            .map(|(position, definition)| symbol(position, &definition.tree))
            .collect();
        let entry = symbols
            .get(pack.entry)
            .ok_or(Error::Pack(Refused::Malformed))?;

        let mut opens = Vec::new();
        for definition in &pack.definitions {
            let mut slots = Vec::new();
            for target in &definition.slots {
                slots.push(match target {
                    Target::Inside(position) => symbols
                        .get(*position)
                        .ok_or(Error::Pack(Refused::Malformed))?
                        .clone(),
                    Target::Outside(hash) => known
                        .get(hash)
                        .ok_or(Error::Pack(Refused::Missing(*hash)))?
                        .clone(),
                });
            }
            opens.push(Arc::new(Open {
                tree: definition.tree.clone(),
                slots,
                lambdas: Vec::new(),
            }));
        }

        let mut received = self.received.borrow_mut();
        received.extend(symbols.iter().cloned().zip(opens.iter().cloned()));
        drop(received);
        let refused = symbols.iter().zip(&opens).find_map(|(symbol, open)| {
            let problem = verify(open, false, &|slot| self.check(slot).ok()).err()?;
            Some(Error::Invalid {
                symbol: symbol.clone(),
                problem,
            })
        });
        match refused {
            None => Ok(entry.clone()),
            Some(error) => {
                let mut received = self.received.borrow_mut();
                for symbol in &symbols {
                    received.remove(symbol);
                }
                Err(error)
            }
        }
    }

    /// The definition a pack brought under `symbol`, if one did.
    pub(super) fn received(&self, symbol: &Symbol) -> Option<Arc<Open>> {
        self.received.borrow().get(symbol).cloned()
    }
}

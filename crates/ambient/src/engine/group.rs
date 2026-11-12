//! Hash: an open definition and the hashes of its slots yield a hash. See
//! `spec/definitions.md`.
//!
//! Definitions that refer to each other cyclically form a group and are
//! hashed as one unit. Finding groups is Tarjan's algorithm over slots, which
//! also finishes every dependency before its dependents.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use super::Engine;
use crate::core::Open;
use crate::error::Error;
use crate::hash::{Hash, Hasher};
use crate::symbol::Symbol;

struct Node {
    symbol: Symbol,
    open: Arc<Open>,
    /// The next slot to visit.
    next: usize,
    index: usize,
    low: usize,
}

impl Engine<'_> {
    /// A definition's hash: its whole content, dependencies included.
    ///
    /// # Errors
    ///
    /// Fails if the definition, or anything it reaches, does not check.
    pub fn hash(&self, symbol: &Symbol) -> Result<Hash, Error> {
        if let Some(hash) = self.hash.borrow().get(symbol) {
            return Ok(*hash);
        }

        // Explicit stacks: a dependency chain can be deeper than the native
        // stack allows. `path` is the chain being explored; `pending` holds
        // finished nodes whose group is not yet complete.
        let mut path = vec![self.node(symbol, 0)?];
        let mut pending: Vec<Node> = Vec::new();
        let mut unfinished: HashMap<Symbol, usize> = HashMap::from([(symbol.clone(), 0)]);
        let mut visited = 1;

        while let Some(node) = path.last_mut() {
            if let Some(target) = node.open.slots.get(node.next) {
                node.next += 1;
                if self.hash.borrow().contains_key(target) {
                    continue;
                }
                if let Some(index) = unfinished.get(target) {
                    node.low = node.low.min(*index);
                } else {
                    let target = target.clone();
                    path.push(self.node(&target, visited)?);
                    unfinished.insert(target, visited);
                    visited += 1;
                }
                continue;
            }

            // Every slot is visited: retreat.
            let Some(node) = path.pop() else { break };
            if let Some(parent) = path.last_mut() {
                parent.low = parent.low.min(node.low);
            }
            let (root, index) = (node.low == node.index, node.index);
            pending.push(node);
            if root {
                // Everything visited after a root and still pending is in
                // its group, and was pushed after anything visited before it.
                let start = pending
                    .iter()
                    .position(|member| member.index >= index)
                    .unwrap_or(0);
                let members: Vec<Node> = pending.drain(start..).collect();
                for member in &members {
                    unfinished.remove(&member.symbol);
                }
                self.close(&members);
            }
        }

        self.hash
            .borrow()
            .get(symbol)
            .copied()
            .ok_or_else(|| Error::UnknownSymbol(symbol.clone()))
    }

    fn node(&self, symbol: &Symbol, index: usize) -> Result<Node, Error> {
        Ok(Node {
            symbol: symbol.clone(),
            open: self.check(symbol)?,
            next: 0,
            index,
            low: index,
        })
    }

    /// Hashes one group, every dependency outside it being hashed already.
    fn close(&self, members: &[Node]) {
        let mut known = self.hash.borrow_mut();
        let encodings: Vec<Vec<u8>> = members.iter().map(|m| m.open.tree.encode()).collect();
        let position = |symbol: &Symbol| members.iter().position(|m| m.symbol == *symbol);

        if let [member] = members
            && !member.open.slots.contains(&member.symbol)
        {
            let mut hasher = Hasher::new("definition");
            hasher.bytes(&encodings[0]);
            for slot in &member.open.slots {
                if let Some(hash) = known.get(slot) {
                    hasher.hash(hash);
                }
            }
            known.insert(member.symbol.clone(), hasher.finish());
            return;
        }

        // Members have no order to begin with, and names must not supply
        // one. Each member gets a colour from its own tree, then repeatedly
        // from its tree and the colours of the members it refers to. After as
        // many rounds as there are members, two members share a colour only
        // if nothing reachable tells them apart, in which case they are the
        // same definition and get the same hash.
        let colour = |member: usize, colours: Option<&[Hash]>| {
            let mut hasher = Hasher::new("member");
            hasher.bytes(&encodings[member]);
            for slot in &members[member].open.slots {
                match (position(slot), colours) {
                    (Some(target), Some(colours)) => hasher.u64(1).hash(&colours[target]),
                    (Some(_), None) => hasher.u64(1),
                    (None, _) => match known.get(slot) {
                        Some(hash) => hasher.u64(0).hash(hash),
                        None => hasher.u64(0),
                    },
                };
            }
            hasher.finish()
        };
        let mut colours: Vec<Hash> = (0..members.len()).map(|m| colour(m, None)).collect();
        for _ in 0..members.len() {
            colours = (0..members.len())
                .map(|m| colour(m, Some(&colours)))
                .collect();
        }

        let order: Vec<Hash> = colours
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut group = Hasher::new("group");
        for colour in &order {
            group.hash(colour);
        }
        let group = group.finish();
        for (member, colour) in members.iter().zip(&colours) {
            let index = order.iter().position(|c| c == colour).unwrap_or(0);
            let hash = Hasher::new("member-of")
                .hash(&group)
                .u64(index as u64)
                .finish();
            known.insert(member.symbol.clone(), hash);
        }
    }
}

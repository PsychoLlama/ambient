//! Hash: an open definition and the hashes of its slots yield a hash. See
//! `spec/definitions.md`.
//!
//! Definitions that refer to each other cyclically form a group and are
//! hashed as one unit. Finding groups is Tarjan's algorithm over slots, which
//! also finishes every dependency before its dependents.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use super::Engine;
use crate::core::{Open, encode_annotations};
use crate::error::Error;
use crate::hash::{Hash, Hasher};
use crate::lower::deep;
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
        deep(|| self.hashed(symbol))
    }

    /// A key for something known of a definition for as long as its
    /// hash and this build of the compiler last: `what` says which
    /// thing. A command keeps what it learned of a program under one.
    ///
    /// # Errors
    ///
    /// Fails as [`Self::hash`] does.
    pub fn known(&self, what: &str, symbol: &Symbol) -> Result<Hash, Error> {
        let hash = self.hash(symbol)?;
        Ok(super::kept(
            &Hasher::new(what).hash(&hash).finish(),
            super::BUILD,
        ))
    }

    /// The hash of a definition that has not been asked about.
    fn hashed(&self, symbol: &Symbol) -> Result<Hash, Error> {
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
    ///
    /// A tree is hashed with each slot replaced by what it refers to, so
    /// neither the numbering of slots nor the order of a union's members,
    /// which names decide, is part of a hash.
    fn close(&self, members: &[Node]) {
        let mut known = self.hash.borrow_mut();
        let position = |symbol: &Symbol| members.iter().position(|m| m.symbol == *symbol);
        let outside = |symbol: &Symbol, out: &mut Vec<u8>| {
            out.push(0);
            if let Some(hash) = known.get(symbol) {
                out.extend_from_slice(hash.as_bytes());
            }
        };

        if let [member] = members
            && !member.open.slots.contains(&member.symbol)
        {
            let slots = &member.open.slots;
            let mut encoding = member
                .open
                .tree
                .resolved(&|slot, out| outside(&slots[slot.0], out));
            encode_annotations(&member.open.annotations, &mut encoding);
            let hash = Hasher::new("definition").bytes(&encoding).finish();
            known.insert(member.symbol.clone(), hash);
            return;
        }

        // Members have no order to begin with, and names must not supply
        // one. Each member gets a colour from its own tree, then repeatedly
        // from its tree and the colours of the members it refers to. After as
        // many rounds as there are members, two members share a colour only
        // if nothing reachable tells them apart, in which case they are the
        // same definition and get the same hash.
        let colour = |member: &Node, colours: Option<&[Hash]>| {
            let mut encoding = member.open.tree.resolved(&|slot, out| {
                let symbol = &member.open.slots[slot.0];
                match position(symbol) {
                    Some(target) => {
                        out.push(1);
                        if let Some(colours) = colours {
                            out.extend_from_slice(colours[target].as_bytes());
                        }
                    }
                    None => outside(symbol, out),
                }
            });
            encode_annotations(&member.open.annotations, &mut encoding);
            Hasher::new("member").bytes(&encoding).finish()
        };
        let mut colours: Vec<Hash> = members.iter().map(|m| colour(m, None)).collect();
        for _ in 0..members.len() {
            colours = members.iter().map(|m| colour(m, Some(&colours))).collect();
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
        let hashes: Vec<Hash> = colours
            .iter()
            .map(|colour| {
                let index = order.iter().position(|c| c == colour).unwrap_or(0);
                Hasher::new("member-of")
                    .hash(&group)
                    .u64(index as u64)
                    .finish()
            })
            .collect();
        for (member, hash) in members.iter().zip(hashes) {
            known.insert(member.symbol.clone(), hash);
        }
    }
}

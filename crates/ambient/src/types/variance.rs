//! How a recursive type definition varies with its type parameters, and
//! whether unfolding it ever ends. See "Subtyping" in `docs/spec/types.md`.
//!
//! A recursive type that applies itself to anything but its own
//! parameters has no end of different unfoldings, so two uses of it
//! cannot be compared by unfolding them. They are compared by their
//! arguments instead, which needs to know which way each argument is
//! held to vary.

use std::collections::{BTreeMap, BTreeSet};

use super::{Env, Ty, mentions_parameter};
use crate::surface::Reference;
use crate::symbol::Symbol;

/// How a type varies with one of its parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variance {
    /// It does not mention the parameter where it matters.
    Unused,
    /// A wider argument gives a wider type.
    Covariant,
    /// A wider argument gives a narrower type.
    Contravariant,
    /// Only the same argument gives a related type.
    Invariant,
}

impl Variance {
    /// How a type varies if it varies both as `self` and as `other`.
    fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unused, variance) | (variance, Self::Unused) => variance,
            (one, other) if one == other => one,
            _ => Self::Invariant,
        }
    }

    /// How a type varies with something it holds in a place that
    /// varies as `self`, if what it holds there varies as `inner`.
    fn of(self, inner: Self) -> Self {
        match (self, inner) {
            (Self::Unused, _) | (_, Self::Unused) => Self::Unused,
            (Self::Invariant, _) | (_, Self::Invariant) => Self::Invariant,
            (one, other) if one == other => Self::Covariant,
            _ => Self::Contravariant,
        }
    }
}

impl Env {
    /// Works out, for every recursive type definition held, how it
    /// varies with each of its parameters and whether unfolding it
    /// ends. Until this has run, none of them is compared by its
    /// arguments.
    pub fn settle(&mut self) {
        let recursive = self.types.iter().filter(|(_, info)| info.nominal());
        // Unions are left as written: simplifying one compares types.
        let bodies: BTreeMap<Symbol, Ty> = recursive
            .map(|(symbol, info)| (symbol.clone(), self.normal(&info.ty, false)))
            .collect();
        for symbol in bodies.keys() {
            if let Some(info) = self.types.get_mut(symbol) {
                info.variance = vec![Variance::Unused; info.bounds.len()];
            }
        }
        // The least answer that holds: each pass can only find more ways
        // a definition varies, and there are four.
        loop {
            let mut changed = false;
            for (symbol, body) in &bodies {
                let mut found = self.types[symbol].variance.clone();
                // An opaque type is held to its arguments, whatever its
                // content does with them: see "Opaque types" in
                // `docs/spec/types.md`.
                if self.types[symbol].opaque {
                    found.fill(Variance::Invariant);
                }
                self.varies(body, Variance::Covariant, &mut found);
                if let Some(info) = self.types.get_mut(symbol)
                    && info.variance != found
                {
                    info.variance = found;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let grows: BTreeSet<&Symbol> = bodies
            .iter()
            .filter(|(_, body)| self.grows(body))
            .map(|(symbol, _)| symbol)
            .collect();
        for symbol in bodies.keys() {
            let ends = !reaches(symbol, &bodies).iter().any(|at| grows.contains(at));
            if let Some(info) = self.types.get_mut(symbol) {
                info.finite = ends;
            }
        }
    }

    /// How a type varies with each of `count` type parameters in scope
    /// where it is written.
    #[must_use]
    pub fn variance(&self, ty: &Ty, count: usize) -> Vec<Variance> {
        let mut found = vec![Variance::Unused; count];
        self.varies(&self.normal(ty, false), Variance::Covariant, &mut found);
        found
    }

    /// How a type holds each of `count` type parameters in scope where
    /// it is written: as [`Self::variance`], but a type match, which
    /// only reads what it tests, holds nothing to more than being read.
    #[must_use]
    pub fn held(&self, ty: &Ty, count: usize) -> Vec<Variance> {
        let mut found = vec![Variance::Unused; count];
        self.vary(
            &self.normal(ty, false),
            Variance::Covariant,
            true,
            &mut found,
        );
        found
    }

    /// Adds to `found` how a type in a place that varies as `at` varies
    /// with each type parameter it mentions.
    fn varies(&self, ty: &Ty, at: Variance, found: &mut [Variance]) {
        self.vary(ty, at, false, found);
    }

    /// [`Self::varies`], and when `reading`, a type match held only to
    /// be read: what it tests for and gives vary as it does.
    fn vary(&self, ty: &Ty, at: Variance, reading: bool, found: &mut [Variance]) {
        let mut all = |types: &[Ty], at: Variance| {
            for ty in types {
                self.vary(ty, at, reading, found);
            }
        };
        match ty {
            Ty::Named(Reference::Parameter(position), _) => {
                if let Some(known) = found.get_mut(*position) {
                    *known = known.join(at);
                }
            }
            // A definition that is not recursive is not here: its name
            // was replaced by what it stands for.
            Ty::Named(Reference::Definition(symbol), args) => {
                let known = self.nominal(symbol).map(|info| info.variance.clone());
                for (position, arg) in args.iter().enumerate() {
                    let inner = known.as_ref().and_then(|known| known.get(position));
                    let inner = inner.copied().unwrap_or(Variance::Invariant);
                    self.vary(arg, at.of(inner), reading, found);
                }
            }
            Ty::Record(fields) => {
                for (_, ty) in fields {
                    self.vary(ty, at, reading, found);
                }
            }
            Ty::Tuple(members) => all(members, at),
            Ty::Union(members) => all(members, at),
            Ty::List(member) | Ty::Iterator(member) | Ty::Branded(member, _) => {
                self.vary(member, at, reading, found);
            }
            Ty::Cell(member) => self.vary(member, at.of(Variance::Invariant), reading, found),
            Ty::Method { needs, function } => {
                self.vary(needs, at.of(Variance::Contravariant), reading, found);
                self.vary(function, at, reading, found);
            }
            Ty::Function { params, ret, .. } => {
                all(params, at.of(Variance::Contravariant));
                self.vary(ret, at, reading, found);
            }
            // Bounds are held to be the same, not one within the other.
            Ty::Generic { bounds, function } => {
                all(bounds, at.of(Variance::Invariant));
                self.vary(function, at, reading, found);
            }
            // A type match left open widens with its subject if each arm
            // widens with its binders, and is otherwise related only to
            // one of the same arms. Read, it holds nothing to more than
            // being read.
            Ty::Match(matching) => {
                let arms = if reading {
                    at
                } else {
                    at.of(Variance::Invariant)
                };
                let subject = if reading || self.widens(matching) {
                    at
                } else {
                    arms
                };
                self.vary(&matching.subject, subject, reading, found);
                for (pattern, ty) in &matching.arms {
                    pattern
                        .tested()
                        .into_iter()
                        .for_each(|tested| self.vary(tested, arms, reading, found));
                    self.vary(ty, arms, reading, found);
                }
            }
            _ => {}
        }
    }

    /// Whether a definition with this body can make new types to
    /// unfold without end: it applies a recursive definition to
    /// something built of a type parameter, and not to the parameter
    /// itself.
    fn grows(&self, body: &Ty) -> bool {
        let grows = std::cell::Cell::new(false);
        let _ = body.rewrite(0, &|ty, _| {
            if let Ty::Named(Reference::Definition(symbol), args) = ty
                && self.nominal(symbol).is_some()
            {
                let bare = |arg: &Ty| matches!(arg, Ty::Named(Reference::Parameter(_), _));
                let built = |arg: &Ty| mentions_parameter(arg) && !bare(arg);
                grows.set(grows.get() || args.iter().any(built));
            }
            None
        });
        grows.get()
    }
}

/// The definitions unfolding `symbol` can come to, itself included.
fn reaches<'a>(symbol: &'a Symbol, bodies: &'a BTreeMap<Symbol, Ty>) -> BTreeSet<&'a Symbol> {
    let mut found = BTreeSet::new();
    let mut pending = vec![symbol];
    while let Some(next) = pending.pop() {
        if !found.insert(next) {
            continue;
        }
        if let Some(body) = bodies.get(next) {
            body.names(&mut |name| {
                if let Reference::Definition(named) = name
                    && let Some((named, _)) = bodies.get_key_value(named)
                {
                    pending.push(named);
                }
            });
        }
    }
    found
}

//! Patterns: what a match arm tests and binds, in a value match or a
//! type match.

use serde::{Deserialize, Serialize};

use super::visit::Leaf;
use super::{Path, Type};
use crate::literal::Literal;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Pattern<N = Path> {
    /// Matches anything.
    Wildcard,
    Literal(Literal),
    /// Matches a record that has at least these fields, each matching.
    Record(Vec<(String, Pattern<N>)>),
    /// Matches a tuple of exactly this length, each member matching.
    Tuple(Vec<Pattern<N>>),
    /// Matches a value the type describes.
    Is(Type<N>),
    /// Matches what the inner pattern matches, and binds it to a name.
    Bind(String, Box<Pattern<N>>),
}

/// A type match: a subject, and arms that each give a type for what
/// their pattern matched of it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TypeMatch<N = Path> {
    pub subject: Type<N>,
    pub arms: Vec<(Pattern<N>, Type<N>)>,
}

impl<N: Clone> Pattern<N> {
    /// The pattern with every type in it rewritten as
    /// [`Type::rewrite`] does.
    #[must_use]
    pub fn retyped(&self, depth: usize, leaf: &Leaf<'_, N>) -> Self {
        match self {
            Self::Wildcard | Self::Literal(_) => self.clone(),
            Self::Record(fields) => Self::Record(
                fields
                    .iter()
                    .map(|(name, pattern)| (name.clone(), pattern.retyped(depth, leaf)))
                    .collect(),
            ),
            Self::Tuple(members) => {
                Self::Tuple(members.iter().map(|p| p.retyped(depth, leaf)).collect())
            }
            Self::Is(ty) => Self::Is(ty.rewrite(depth, leaf)),
            Self::Bind(name, inner) => {
                Self::Bind(name.clone(), Box::new(inner.retyped(depth, leaf)))
            }
        }
    }
}

impl<N> Pattern<N> {
    /// The types the pattern tests for, in order.
    #[must_use]
    pub fn tested(&self) -> Vec<&Type<N>> {
        match self {
            Self::Wildcard | Self::Literal(_) => Vec::new(),
            Self::Is(ty) => vec![ty],
            Self::Bind(_, inner) => inner.tested(),
            Self::Tuple(members) => members.iter().flat_map(Self::tested).collect(),
            Self::Record(fields) => fields.iter().flat_map(|(_, p)| p.tested()).collect(),
        }
    }

    /// Calls `visit` with every name in the pattern's types, in order.
    pub fn names<'a>(&'a self, visit: &mut impl FnMut(&'a N)) {
        match self {
            Self::Wildcard | Self::Literal(_) => {}
            Self::Record(fields) => fields.iter().for_each(|(_, p)| p.names(visit)),
            Self::Tuple(members) => members.iter().for_each(|p| p.names(visit)),
            Self::Is(ty) => ty.names(visit),
            Self::Bind(_, inner) => inner.names(visit),
        }
    }

    /// How many names the pattern binds.
    #[must_use]
    pub fn binders(&self) -> usize {
        match self {
            Self::Wildcard | Self::Literal(_) | Self::Is(_) => 0,
            Self::Record(fields) => fields.iter().map(|(_, p)| p.binders()).sum(),
            Self::Tuple(members) => members.iter().map(Self::binders).sum(),
            Self::Bind(_, inner) => 1 + inner.binders(),
        }
    }
}

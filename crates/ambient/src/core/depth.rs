//! How deeply a core tree nests, and the limit on it. See "Limits" in
//! `spec/core.md`.
//!
//! Everything that walks a tree recurses once per level, so the limit is
//! what lets [`crate::lower::deep`] promise room for any tree. A tree is
//! held to it wherever one is made: by the checker, and here, as one is
//! decoded.

use std::cell::Cell;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

use super::{Expr, Function, Open, Pattern, Tree, Type};

/// The most levels of expressions, types, and patterns a tree nests.
pub const DEPTH: usize = 4096;

thread_local! {
    /// How many nodes the decoding under way is inside.
    static INSIDE: Cell<usize> = const { Cell::new(0) };
}

/// One more level of the tree being decoded, until it is dropped.
struct Level;

impl Level {
    fn enter<E: Error>() -> Result<Self, E> {
        INSIDE.with(|inside| {
            if inside.get() >= DEPTH {
                return Err(E::custom("a tree nests too deeply"));
            }
            inside.set(inside.get() + 1);
            Ok(Self)
        })
    }
}

impl Drop for Level {
    fn drop(&mut self) {
        INSIDE.with(|inside| inside.set(inside.get() - 1));
    }
}

/// Gives a node the encoding it derives, as `serde(remote = "Self")`
/// leaves it, and counts a level around decoding one.
macro_rules! counted {
    ($node:ty) => {
        impl Serialize for $node {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                <$node>::serialize(self, serializer)
            }
        }

        impl<'de> Deserialize<'de> for $node {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let _level = Level::enter()?;
                <$node>::deserialize(deserializer)
            }
        }
    };
}

counted!(Expr);
counted!(Type);
counted!(Pattern);

fn deepest<'a>(types: impl IntoIterator<Item = &'a Type>) -> usize {
    types.into_iter().map(Type::depth).max().unwrap_or(0)
}

impl Type {
    /// How many levels the type nests, itself included.
    #[must_use]
    pub fn depth(&self) -> usize {
        1 + match self {
            Self::Record(fields) => deepest(fields.iter().map(|(_, ty)| ty)),
            Self::Tuple(members) | Self::Union(members) | Self::Reference(_, members) => {
                deepest(members)
            }
            Self::List(member) | Self::Cell(member) | Self::Iterator(member) => member.depth(),
            Self::Function { params, ret, .. } => deepest(params).max(ret.depth()),
            Self::Generic { bounds, function } => deepest(bounds).max(function.depth()),
            _ => 0,
        }
    }
}

impl Pattern {
    fn depth(&self) -> usize {
        let deepest = |patterns: &mut dyn Iterator<Item = &Self>| {
            patterns.map(Self::depth).max().unwrap_or(0)
        };
        1 + match self {
            Self::Wildcard | Self::Literal(_) => 0,
            Self::Record(fields) => deepest(&mut fields.iter().map(|(_, inner)| inner)),
            Self::Tuple(members) => deepest(&mut members.iter()),
            Self::Is(ty) => ty.depth(),
            Self::Bind(inner) => inner.depth(),
        }
    }
}

impl Expr {
    /// How deeply the types and patterns written in this node nest.
    fn written(&self) -> usize {
        match self {
            Self::Reference(_, types)
            | Self::Closure {
                type_args: types, ..
            } => deepest(types),
            Self::Cast { ty, .. }
            | Self::Cell { ty, .. }
            | Self::Widen { ty, .. }
            | Self::Brand { ty, .. } => ty.depth(),
            Self::Match { arms, .. } => {
                let patterns = arms.iter().map(|(pattern, _)| pattern.depth());
                patterns.max().unwrap_or(0)
            }
            _ => 0,
        }
    }

    /// How many levels the expression nests, itself included.
    #[must_use]
    pub fn depth(&self) -> usize {
        let mut deepest = 0;
        let mut pending = vec![(self, 1)];
        while let Some((expr, depth)) = pending.pop() {
            deepest = deepest.max(depth + expr.written());
            expr.children(&mut |child| pending.push((child, depth + 1)));
        }
        deepest
    }
}

impl Tree {
    /// How many levels the definition nests.
    #[must_use]
    pub fn depth(&self) -> usize {
        match self {
            Self::Type { ty, .. } | Self::Key { ty, .. } => ty.depth(),
            Self::Function(Function {
                type_params,
                captures,
                params,
                ret,
                body,
                ..
            }) => deepest(type_params.iter().chain(captures).chain(params))
                .max(ret.depth())
                .max(body.depth()),
            Self::Ability { params, ret, .. } | Self::Extern { params, ret, .. } => {
                deepest(params).max(ret.depth())
            }
            Self::Target { .. } => 0,
        }
    }
}

impl Open {
    /// Whether the definition and the lambdas written in it nest no
    /// deeper than [`DEPTH`].
    #[must_use]
    pub fn shallow(&self) -> bool {
        let mut pending = vec![self];
        while let Some(open) = pending.pop() {
            if open.tree.depth() > DEPTH {
                return false;
            }
            pending.extend(&open.lambdas);
        }
        true
    }
}

//! Names: a path as it is written, and what one means once resolved.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::symbol::Symbol;

/// A dotted name as written: `x`, `math.add`, `pkg.util.math`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Path(pub Vec<String>);

impl Path {
    /// Splits a dotted name into its segments.
    #[must_use]
    pub fn parse(dotted: &str) -> Self {
        Self(dotted.split('.').map(str::to_owned).collect())
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.join("."))
    }
}

/// What a name means once resolved.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Reference {
    /// A parameter or local binder, numbered in binding order within its
    /// function.
    Local(usize),
    /// A type parameter of the enclosing definition, by position.
    Parameter(usize),
    Definition(Symbol),
}

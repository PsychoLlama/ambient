//! Literals: the values that can be written directly, and so have a type of
//! their own. See `spec/types.md`.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Literal {
    Bool(bool),
    Int(i64),
    /// The bits of a 64-bit float, so that literals compare exactly.
    Float(u64),
    String(String),
}

impl Literal {
    #[must_use]
    pub fn float(value: f64) -> Self {
        Self::Float(value.to_bits())
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(value) => write!(f, "{value}"),
            Self::Int(value) => write!(f, "{value}"),
            Self::Float(bits) => write!(f, "{:?}", f64::from_bits(*bits)),
            Self::String(value) => write!(f, "{value:?}"),
        }
    }
}

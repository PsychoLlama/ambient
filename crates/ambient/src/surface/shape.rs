//! The built-in shapes that have methods. See `spec/methods.md`.

use serde::{Deserialize, Serialize};

/// A shape of value that has methods of the standard library: every
/// built-in shape but a record. See `spec/methods.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Shape {
    Null,
    Bool,
    Int,
    Float,
    String,
    Tag,
    Tuple,
    List,
    Function,
    Cell,
    Iterator,
}

impl Shape {
    pub const ALL: [Self; 11] = [
        Self::Null,
        Self::Bool,
        Self::Int,
        Self::Float,
        Self::String,
        Self::Tag,
        Self::Tuple,
        Self::List,
        Self::Function,
        Self::Cell,
        Self::Iterator,
    ];

    /// The module of the standard library whose functions are the
    /// shape's methods.
    #[must_use]
    pub fn module(self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Float => "float",
            Self::String => "string",
            Self::Tag => "tag",
            Self::Tuple => "tuple",
            Self::List => "list",
            Self::Function => "function",
            Self::Cell => "cell",
            Self::Iterator => "iterator",
        }
    }
}

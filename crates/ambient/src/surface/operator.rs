//! Operators. See `spec/operations.md`.

use serde::{Deserialize, Serialize};

/// An operator. Which primitive operation it means depends on its operands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Less,
    Equal,
    /// How long a string or a list is.
    Length,
    /// The member of a list at a position.
    Get,
    /// A list with one more member.
    Push,
    /// One string or list followed by another.
    Join,
    /// What lies between two positions of a string or a list.
    Slice,
    /// A float or a string as an integer.
    ToInt,
    /// An integer or a string as a float.
    ToFloat,
    /// A number as a string.
    ToText,
    /// An iterator that a function steps.
    Iterator,
    /// An iterator over the members of a list.
    Iterate,
    /// An iterator over the integers from one up to another.
    Range,
    /// What an iterator gives next, and the iterator of the rest.
    Step,
}

impl Operator {
    /// The operator as source text writes it.
    #[must_use]
    pub fn written(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::Divide => "/",
            Self::Remainder => "%",
            Self::Less => "<",
            Self::Equal => "==",
            Self::Join => "++",
            Self::Length => "@length",
            Self::Get => "@get",
            Self::Push => "@push",
            Self::Slice => "@slice",
            Self::ToInt => "@to_int",
            Self::ToFloat => "@to_float",
            Self::ToText => "@to_text",
            Self::Iterator => "@iterator",
            Self::Iterate => "@iterate",
            Self::Range => "@range",
            Self::Step => "@step",
        }
    }
}

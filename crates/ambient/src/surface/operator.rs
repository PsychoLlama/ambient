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
    /// How long a string, a list, or a bytes value is.
    Length,
    /// The member of a list, or the byte of a bytes value, at a position.
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
    /// A number as a string, or the string some bytes encode.
    ToText,
    /// A string's encoding, or a list of integers, as bytes.
    ToBytes,
    /// The scalar value of a character.
    Code,
    /// The character of a scalar value.
    Character,
    /// An iterator that a function steps.
    Iterator,
    /// An iterator over the members of a list.
    Iterate,
    /// An iterator over the integers from one up to another.
    Range,
    /// What an iterator gives next, and the iterator of the rest.
    Step,
    /// What each of a list of functions gives, run as tasks.
    Gather,
    /// What the first of a list of functions to be over gives.
    Race,
}

impl Operator {
    /// Every operator.
    pub const ALL: [Self; 24] = [
        Self::Add,
        Self::Subtract,
        Self::Multiply,
        Self::Divide,
        Self::Remainder,
        Self::Less,
        Self::Equal,
        Self::Length,
        Self::Get,
        Self::Push,
        Self::Join,
        Self::Slice,
        Self::ToInt,
        Self::ToFloat,
        Self::ToText,
        Self::ToBytes,
        Self::Code,
        Self::Character,
        Self::Iterator,
        Self::Iterate,
        Self::Range,
        Self::Step,
        Self::Gather,
        Self::Race,
    ];

    /// The word for the operator: what `@word(...)` writes it by, and
    /// what a tree of code names it by.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Subtract => "subtract",
            Self::Multiply => "multiply",
            Self::Divide => "divide",
            Self::Remainder => "remainder",
            Self::Less => "less",
            Self::Equal => "equal",
            Self::Length => "length",
            Self::Get => "get",
            Self::Push => "push",
            Self::Join => "join",
            Self::Slice => "slice",
            Self::ToInt => "to_int",
            Self::ToFloat => "to_float",
            Self::ToText => "to_text",
            Self::ToBytes => "to_bytes",
            Self::Code => "code",
            Self::Character => "character",
            Self::Iterator => "iterator",
            Self::Iterate => "iterate",
            Self::Range => "range",
            Self::Step => "step",
            Self::Gather => "gather",
            Self::Race => "race",
        }
    }

    /// The operator a word is for, if it is for one.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|operator| operator.name() == word)
    }

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
            Self::ToBytes => "@to_bytes",
            Self::Code => "@code",
            Self::Character => "@character",
            Self::Iterator => "@iterator",
            Self::Iterate => "@iterate",
            Self::Range => "@range",
            Self::Step => "@step",
            Self::Gather => "@gather",
            Self::Race => "@race",
        }
    }
}

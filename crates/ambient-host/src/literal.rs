//! Literals: the values that can be written directly, and so have a type of
//! their own. See `docs/spec/types.md`.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::value::Value;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Literal {
    Bool(bool),
    Int(i64),
    /// The bits of a 64-bit float, so that literals compare exactly.
    Float(u64),
    String(String),
    Bytes(Vec<u8>),
    /// A tag: a token equal only to the same tag of the same package.
    /// The package is the one the tag is written in, which resolution
    /// fills in; a builder leaves it empty.
    Tag {
        package: String,
        tag: String,
    },
}

/// A brand: a mark on a type, told from every other by where it is
/// declared. See "Brands" in `docs/spec/types.md`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Brand {
    /// The package it is declared in.
    pub package: String,
    /// Its identity: the module path and name it is declared under.
    pub brand: String,
}

impl Literal {
    #[must_use]
    pub fn float(value: f64) -> Self {
        Self::Float(value.to_bits())
    }

    /// The value the literal is.
    #[must_use]
    pub fn value(&self) -> Value {
        match self {
            Self::Bool(value) => Value::Bool(*value),
            Self::Int(value) => Value::Int(*value),
            Self::Float(bits) => Value::Float(f64::from_bits(*bits)),
            Self::String(value) => Value::String(value.clone()),
            Self::Bytes(value) => Value::Bytes(value.clone()),
            Self::Tag { package, tag } => Value::Tag {
                package: package.clone(),
                tag: tag.clone(),
            },
        }
    }

    /// What a tag is to the runtime: its package and itself, as one text.
    #[must_use]
    pub fn tag_text(package: &str, tag: &str) -> String {
        format!("{package}\0{tag}")
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(value) => write!(f, "{value}"),
            Self::Int(value) => write!(f, "{value}"),
            Self::Float(bits) => write!(f, "{:?}", f64::from_bits(*bits)),
            Self::String(value) => write!(f, "{value:?}"),
            Self::Bytes(bytes) => {
                let pairs: Vec<String> = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
                write!(f, "0x[{}]", pairs.join(" "))
            }
            Self::Tag { tag, .. } => write!(f, "#{tag}"),
        }
    }
}

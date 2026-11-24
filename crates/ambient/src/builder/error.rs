//! Why a builder refused a tree.

use std::fmt;

use crate::surface::Path;

/// Why a builder refused a tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildError {
    /// Not an identifier: empty, or contains something other than letters,
    /// digits, and underscores, or starts with a digit.
    InvalidName(String),
    /// A module defines a name twice in one namespace, or imports two
    /// things under one alias.
    DuplicateDefinition(String),
    /// A constant is not named in capitals.
    ConstantName(String),
    /// A module whose content is a comptime call defines something
    /// besides.
    GeneratedWithMore,
    DuplicateTypeParameter(String),
    DuplicateParameter {
        function: String,
        name: String,
    },
    DuplicateField(String),
    /// A comptime call must be a call to a function by name.
    ComptimeNotCall,
    /// One record, or one target, holds two handlers for an ability.
    DuplicateHandler(Path),
    /// One pattern binds a name twice.
    DuplicateBinder(String),
    /// A tag is written by its text. One is declared and then named.
    WrittenTag(String),
    /// A tuple, or an all, of fewer than two members.
    ShortTuple,
    /// A mark of the reader's was left in a tree.
    Mark,
    /// An expression nests more deeply than any tree may.
    Nesting,
    /// Type parameters are given to something other than a function
    /// type, or one is named outside the function type that has it.
    GenericType,
    /// A type match has no arms, or a type names a binder of an arm it
    /// is not inside.
    TypeMatch,
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(f, "`{name}` is not a name"),
            Self::DuplicateDefinition(name) => write!(f, "`{name}` is defined twice"),
            Self::ConstantName(name) => write!(
                f,
                "the constant `{name}` is not named in capital letters, digits, and `_`"
            ),
            Self::GeneratedWithMore => write!(
                f,
                "a module that is a comptime call defines nothing else: only its imports are written"
            ),
            Self::DuplicateParameter { function, name } => {
                write!(f, "`{function}` has two parameters named `{name}`")
            }
            Self::DuplicateTypeParameter(name) => {
                write!(f, "two type parameters are named `{name}`")
            }
            Self::DuplicateField(name) => write!(f, "a record has two fields named `{name}`"),
            Self::ComptimeNotCall => {
                write!(f, "a comptime call must call a function by name")
            }
            Self::DuplicateHandler(path) => write!(f, "two handlers for `{path}`"),
            Self::DuplicateBinder(name) => write!(f, "a pattern binds `{name}` twice"),
            Self::ShortTuple => write!(f, "a tuple has two or more members"),
            Self::GenericType => write!(
                f,
                "only a function type has type parameters, named only inside it"
            ),
            Self::TypeMatch => write!(
                f,
                "a type match has arms, and a binder is named only in its arm"
            ),
            Self::WrittenTag(tag) => {
                write!(f, "the tag `{tag}` is written out: declare it and name it")
            }
            Self::Mark => write!(f, "a mark of the reader's was left in a tree"),
            Self::Nesting => write!(
                f,
                "an expression nests more than {} levels deep",
                crate::core::DEPTH
            ),
        }
    }
}

impl std::error::Error for BuildError {}

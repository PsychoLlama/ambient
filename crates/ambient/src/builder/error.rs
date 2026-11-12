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
    /// An import must name a package and at least one thing inside it.
    ImportTooShort(Path),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(f, "`{name}` is not a name"),
            Self::DuplicateDefinition(name) => write!(f, "`{name}` is defined twice"),
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
            Self::ImportTooShort(path) => write!(f, "import `{path}` names only a package"),
        }
    }
}

impl std::error::Error for BuildError {}

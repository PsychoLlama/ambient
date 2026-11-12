//! Everything a stage can report about a program.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::surface::Path;
use crate::symbol::{ModuleId, Symbol};

/// A problem in the program being built.
///
/// Errors are stage results like any other: they are stored, and a stage that
/// failed is not re-run until one of its inputs changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Error {
    /// No module exists at this path.
    UnknownModule(ModuleId),
    /// The module exists but defines no such symbol.
    UnknownSymbol(Symbol),
    /// An import's first segment is neither `pkg` nor a declared dependency.
    UnknownPackage { module: ModuleId, name: String },
    /// An import names nothing.
    UnknownImport { module: ModuleId, path: Path },
    /// An import names a definition that is private to its module.
    PrivateImport { module: ModuleId, path: Path },
    /// Two things in one module's scope share a name.
    DuplicateName { module: ModuleId, name: String },
    /// A name in a definition resolves to nothing.
    UnknownName { symbol: Symbol, path: Path },
    /// A name reached through a module is private to that module.
    PrivateName { symbol: Symbol, path: Path },
    /// A call passes the wrong number of arguments.
    Arity {
        symbol: Symbol,
        callee: Symbol,
        expected: usize,
        found: usize,
    },
    /// An expression's type is not a subtype of the type required of it.
    Mismatch {
        symbol: Symbol,
        expected: String,
        found: String,
    },
    /// Something that is not a function is called.
    NotCallable { symbol: Symbol, found: String },
    /// A field is read from a type that may lack it.
    NoField {
        symbol: Symbol,
        ty: String,
        name: String,
    },
    /// An operator is applied to types it is not defined for.
    Operands { symbol: Symbol, found: String },
    /// A match leaves some values of its subject unhandled.
    NotExhaustive { symbol: Symbol, remaining: String },
    /// A match arm can never be taken.
    UnreachableArm { symbol: Symbol, arm: usize },
    /// A recursive type that never reaches a constructor.
    Uncontractive(Symbol),
    /// Only functions compile to an artifact.
    NotCompilable(Symbol),
    /// A backend refused a definition.
    Backend { symbol: Symbol, message: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownModule(module) => write!(f, "no module `{module}`"),
            Self::UnknownSymbol(symbol) => write!(f, "no definition `{symbol}`"),
            Self::UnknownPackage { module, name } => {
                write!(f, "{module}: `{name}` is not a dependency")
            }
            Self::UnknownImport { module, path } => {
                write!(f, "{module}: import `{path}` names nothing")
            }
            Self::PrivateImport { module, path } => {
                write!(f, "{module}: import `{path}` is private")
            }
            Self::DuplicateName { module, name } => {
                write!(f, "{module}: `{name}` is bound twice")
            }
            Self::UnknownName { symbol, path } => {
                write!(f, "{symbol}: `{path}` names nothing")
            }
            Self::PrivateName { symbol, path } => write!(f, "{symbol}: `{path}` is private"),
            Self::Arity {
                symbol,
                callee,
                expected,
                found,
            } => write!(
                f,
                "{symbol}: `{callee}` takes {expected} arguments, found {found}"
            ),
            Self::Mismatch {
                symbol,
                expected,
                found,
            } => write!(f, "{symbol}: expected `{expected}`, found `{found}`"),
            Self::NotCallable { symbol, found } => {
                write!(f, "{symbol}: `{found}` is not a function")
            }
            Self::NoField { symbol, ty, name } => {
                write!(f, "{symbol}: `{ty}` may have no field `{name}`")
            }
            Self::Operands { symbol, found } => {
                write!(f, "{symbol}: no operator for `{found}`")
            }
            Self::NotExhaustive { symbol, remaining } => {
                write!(f, "{symbol}: match does not handle `{remaining}`")
            }
            Self::UnreachableArm { symbol, arm } => {
                write!(f, "{symbol}: match arm {arm} can never be taken")
            }
            Self::Uncontractive(symbol) => {
                write!(
                    f,
                    "{symbol}: a recursive type must pass through a constructor"
                )
            }
            Self::NotCompilable(symbol) => write!(f, "{symbol}: not a function"),
            Self::Backend { symbol, message } => write!(f, "{symbol}: {message}"),
        }
    }
}

impl std::error::Error for Error {}

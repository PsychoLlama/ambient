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
    /// A type or function is given the wrong number of type arguments.
    TypeArguments {
        symbol: Symbol,
        expected: usize,
        found: usize,
    },
    /// A generic function is used where its type arguments cannot be
    /// worked out.
    CannotInfer { symbol: Symbol, function: Symbol },
    /// A comptime argument's value is not known from its type.
    NotComptime { symbol: Symbol, found: String },
    /// A comptime call's result has no literal form.
    NotSpliceable { symbol: Symbol },
    /// A comptime call faulted.
    ComptimeFault { function: Symbol, fault: String },
    /// A definition's content depends on a comptime call that depends on
    /// the definition.
    ComptimeCycle(Symbol),
    /// A provide is given something that is not a record of handlers.
    NotBundle { symbol: Symbol, found: String },
    /// A record holds two handlers for one ability.
    DuplicateHandler { symbol: Symbol, ability: Symbol },
    /// An ability is used where nothing binds it.
    Unbound { symbol: Symbol, ability: Symbol },
    /// A comptime call names a function that takes abilities.
    ComptimeAbilities { symbol: Symbol, function: Symbol },
    /// A program's entry takes abilities, and nothing provides them.
    EntryAbilities(Symbol),
    /// A lambda parameter has no type, and nothing expects one of it.
    UntypedParameter { symbol: Symbol, name: String },
    /// A recursive generic type applies itself to something other than its
    /// own parameters.
    IrregularType(Symbol),
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
            Self::UnknownModule(_)
            | Self::UnknownSymbol(_)
            | Self::UnknownPackage { .. }
            | Self::UnknownImport { .. }
            | Self::PrivateImport { .. }
            | Self::DuplicateName { .. }
            | Self::UnknownName { .. }
            | Self::PrivateName { .. } => self.fmt_name(f),
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
            Self::TypeArguments {
                symbol,
                expected,
                found,
            } => write!(
                f,
                "{symbol}: expected {expected} type arguments, found {found}"
            ),
            Self::CannotInfer { symbol, function } => {
                write!(
                    f,
                    "{symbol}: cannot infer the type arguments of `{function}`"
                )
            }
            Self::NotComptime { symbol, found } => write!(
                f,
                "{symbol}: a comptime argument must have one possible value, found `{found}`"
            ),
            Self::NotSpliceable { symbol } => {
                write!(f, "{symbol}: a comptime result must be plain data")
            }
            Self::ComptimeFault { function, fault } => {
                write!(f, "`{function}` faulted during compilation: {fault}")
            }
            Self::ComptimeCycle(symbol) => {
                write!(f, "{symbol}: depends on a comptime call that depends on it")
            }
            Self::NotBundle { symbol, found } => {
                write!(f, "{symbol}: `{found}` is not a record of handlers")
            }
            Self::DuplicateHandler { symbol, ability } => {
                write!(f, "{symbol}: two handlers for `{ability}`")
            }
            Self::Unbound { symbol, ability } => {
                write!(f, "{symbol}: `{ability}` is not bound here")
            }
            Self::ComptimeAbilities { symbol, function } => write!(
                f,
                "{symbol}: `{function}` takes abilities, which a comptime call cannot provide"
            ),
            Self::EntryAbilities(symbol) => {
                write!(f, "{symbol}: an entry cannot take abilities yet")
            }
            Self::UntypedParameter { symbol, name } => {
                write!(f, "{symbol}: parameter `{name}` needs a type")
            }
            Self::IrregularType(symbol) => write!(
                f,
                "{symbol}: a recursive type may apply itself only to its own parameters"
            ),
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

impl Error {
    /// The errors about names: what reading and resolving report.
    fn fmt_name(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
            _ => Ok(()),
        }
    }
}

impl std::error::Error for Error {}

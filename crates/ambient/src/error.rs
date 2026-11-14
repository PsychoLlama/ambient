//! Everything a stage can report about a program.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::pack::Refused;
use crate::surface::Path;
use crate::symbol::{ModuleId, Symbol};
use crate::verify::Invalid;

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
    /// A module re-exports a module. Only definitions are re-exported.
    ExportsModule { module: ModuleId, path: Path },
    /// Modules re-export from each other in a circle.
    ExportCycle(ModuleId),
    /// An import reaches into a module of another package that its
    /// library module does not list.
    InternalModule { module: ModuleId, path: Path },
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
    /// A record holds under a key a value that is not of the key's
    /// type.
    KeyType {
        symbol: Symbol,
        name: String,
        found: String,
    },
    /// A method is called on a value whose type has none of that name.
    NoMethod {
        symbol: Symbol,
        ty: String,
        name: String,
    },
    /// A method is not a field of a record, or is not a function with a
    /// parameter for its record.
    Method { symbol: Symbol, found: String },
    /// A record does not have what one of its methods needs of it.
    Receiver {
        symbol: Symbol,
        name: String,
        needs: String,
        found: String,
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
    /// An ability that does not return takes type parameters.
    GenericAbort(Symbol),
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
    /// A name that is not a value is used as one: a target, or a native
    /// handler, which only a target may name.
    NotValue { symbol: Symbol, name: Symbol },
    /// Something that is not a target is used as one.
    NotTarget(Symbol),
    /// A target binds an ability to something that cannot handle it.
    NotHandler { symbol: Symbol, handler: Symbol },
    /// A target extends itself, or one of its handlers needs itself.
    TargetCycle(Symbol),
    /// Two targets a target extends bind one ability differently, and it
    /// does not say which it wants.
    TargetConflict { symbol: Symbol, ability: Symbol },
    /// A program's entry has type parameters.
    GenericEntry(Symbol),
    /// An ability is used where nothing binds it.
    Unbound { symbol: Symbol, ability: Symbol },
    /// A spawn is written outside any group, or inside a function inside
    /// one.
    SpawnOutsideGroup(Symbol),
    /// A spawned task uses an ability bound inside its group, which may
    /// be over before the task is.
    SpawnBinding { symbol: Symbol, ability: Symbol },
    /// A comptime call takes a build ability that the package it is
    /// written in was not granted.
    Ungranted { symbol: Symbol, ability: Symbol },
    /// A comptime call names a function that takes abilities, and the
    /// build has no target to provide them.
    ComptimeAbilities(Symbol),
    /// A program's entry takes abilities, and nothing provides them.
    EntryAbilities(Symbol),
    /// A lambda parameter has no type, and nothing expects one of it.
    UntypedParameter { symbol: Symbol, name: String },
    /// A recursive type that never reaches a constructor.
    Uncontractive(Symbol),
    /// A cast or a pattern tests for a function type that mentions a
    /// type parameter, which no function says it is of.
    Untestable { symbol: Symbol, found: String },
    /// A definition nests more deeply than any tree may.
    Nesting(Symbol),
    /// Only functions compile to an artifact.
    NotCompilable(Symbol),
    /// A backend refused a definition.
    Backend { symbol: Symbol, message: String },
    /// A core tree is not one checking could have made.
    Invalid { symbol: Symbol, problem: Invalid },
    /// A pack cannot be taken in.
    Pack(Refused),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownModule(_)
            | Self::UnknownSymbol(_)
            | Self::UnknownPackage { .. }
            | Self::UnknownImport { .. }
            | Self::PrivateImport { .. }
            | Self::InternalModule { .. }
            | Self::ExportsModule { .. }
            | Self::ExportCycle(_)
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
            Self::Operands { symbol, found } => {
                write!(f, "{symbol}: no operator for `{found}`")
            }
            Self::NotCallable { .. }
            | Self::NoField { .. }
            | Self::KeyType { .. }
            | Self::NoMethod { .. }
            | Self::Method { .. }
            | Self::Receiver { .. } => self.fmt_method(f),
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
            Self::GenericAbort(_) | Self::CannotInfer { .. } => self.fmt_generic(f),
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
            Self::NotValue { .. }
            | Self::NotTarget(_)
            | Self::NotHandler { .. }
            | Self::TargetCycle(_)
            | Self::TargetConflict { .. }
            | Self::GenericEntry(_) => self.fmt_world(f),
            Self::NotBundle { .. }
            | Self::DuplicateHandler { .. }
            | Self::Unbound { .. }
            | Self::SpawnOutsideGroup(_)
            | Self::SpawnBinding { .. }
            | Self::Ungranted { .. }
            | Self::ComptimeAbilities(_)
            | Self::EntryAbilities(_) => self.fmt_ability(f),
            Self::UntypedParameter { symbol, name } => {
                write!(f, "{symbol}: parameter `{name}` needs a type")
            }
            Self::Untestable { .. } | Self::Uncontractive(_) => self.fmt_type(f),
            Self::Nesting(symbol) => write!(
                f,
                "{symbol}: nests more than {} levels deep",
                crate::core::DEPTH
            ),
            Self::NotCompilable(symbol) => write!(f, "{symbol}: not a function"),
            Self::Backend { symbol, message } => write!(f, "{symbol}: {message}"),
            Self::Invalid { symbol, problem } => write!(f, "{symbol}: {problem}"),
            Self::Pack(refused) => write!(f, "{refused}"),
        }
    }
}

impl Error {
    /// The errors about what a type may be.
    fn fmt_type(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Untestable { symbol, found } => write!(
                f,
                "{symbol}: no value can be tested for `{found}`: a function type in it mentions a type parameter"
            ),
            Self::Uncontractive(symbol) => {
                write!(
                    f,
                    "{symbol}: a recursive type must pass through a constructor"
                )
            }
            _ => Ok(()),
        }
    }

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
            Self::ExportsModule { module, path } => {
                write!(
                    f,
                    "{module}: `{path}` is a module, and cannot be re-exported"
                )
            }
            Self::ExportCycle(module) => {
                write!(f, "{module}: its re-exports lead back to itself")
            }
            Self::InternalModule { module, path } => write!(
                f,
                "{module}: import `{path}` reaches a module its package does not expose"
            ),
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

impl Error {
    /// The errors about abilities and what binds them.
    fn fmt_ability(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBundle { symbol, found } => {
                write!(f, "{symbol}: `{found}` is not a record of handlers")
            }
            Self::DuplicateHandler { symbol, ability } => {
                write!(f, "{symbol}: two handlers for `{ability}`")
            }
            Self::Unbound { symbol, ability } => {
                write!(f, "{symbol}: `{ability}` is not bound here")
            }
            Self::SpawnOutsideGroup(symbol) => {
                write!(
                    f,
                    "{symbol}: a spawn must be written in the body of a group"
                )
            }
            Self::SpawnBinding { symbol, ability } => write!(
                f,
                "{symbol}: a spawned task cannot use `{ability}`, which is bound inside its group"
            ),
            Self::Ungranted { symbol, ability } => write!(
                f,
                "{symbol}: a comptime call takes `{ability}`, which its package is not granted"
            ),
            Self::ComptimeAbilities(function) => write!(
                f,
                "`{function}` takes abilities, and the build has no target to provide them"
            ),
            Self::EntryAbilities(symbol) => {
                write!(f, "{symbol}: an entry cannot take abilities yet")
            }
            _ => Ok(()),
        }
    }

    /// The errors about targets, externs, and entries.
    fn fmt_world(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotValue { symbol, name } => write!(f, "{symbol}: `{name}` is not a value"),
            Self::NotTarget(symbol) => write!(f, "{symbol}: not a target"),
            Self::NotHandler { symbol, handler } => {
                write!(f, "{symbol}: `{handler}` cannot be bound as a handler")
            }
            Self::TargetCycle(symbol) => {
                write!(f, "{symbol}: a target or one of its handlers needs itself")
            }
            Self::TargetConflict { symbol, ability } => write!(
                f,
                "{symbol}: the targets it extends bind `{ability}` differently"
            ),
            Self::GenericEntry(symbol) => {
                write!(f, "{symbol}: an entry cannot have type parameters")
            }
            _ => Ok(()),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// The errors about type parameters and type arguments.
    fn fmt_generic(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GenericAbort(symbol) => write!(
                f,
                "{symbol}: an ability that does not return takes no type parameters"
            ),
            Self::CannotInfer { symbol, function } => write!(
                f,
                "{symbol}: cannot infer the type arguments of `{function}`"
            ),
            _ => Ok(()),
        }
    }

    fn fmt_method(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotCallable { symbol, found } => {
                write!(f, "{symbol}: `{found}` is not a function")
            }
            Self::NoField { symbol, ty, name } => {
                write!(f, "{symbol}: `{ty}` may have no field `{name}`")
            }
            Self::KeyType {
                symbol,
                name,
                found,
            } => write!(f, "{symbol}: `{found}` is not what the key `{name}` holds"),
            Self::NoMethod { symbol, ty, name } => {
                write!(f, "{symbol}: `{ty}` has no method `{name}`")
            }
            Self::Method { symbol, found } => write!(
                f,
                "{symbol}: a method is a field of a record and a function that takes \
                 the record first, not `{found}`"
            ),
            Self::Receiver {
                symbol,
                name,
                needs,
                found,
            } => write!(
                f,
                "{symbol}: method `{name}` needs `{needs}` of its record, which is `{found}`"
            ),
            _ => Ok(()),
        }
    }
}

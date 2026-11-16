//! What each error says to whoever wrote the program.

use std::fmt;

use super::Error;

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
            | Self::PrivateName { .. }
            | Self::NotPerformed { .. } => self.fmt_name(f),
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
            Self::Operands {
                symbol,
                operator,
                found,
            } => {
                write!(f, "{symbol}: `{operator}` is not defined for `{found}`")
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
            Self::NotComptime { .. }
            | Self::NoTree { .. }
            | Self::NotSpliceable { .. }
            | Self::ComptimeFault { .. }
            | Self::NotType(_)
            | Self::NotCode(_)
            | Self::Unnameable { .. }
            | Self::Reaches { .. }
            | Self::ComptimeCycle(_) => self.fmt_comptime(f),
            Self::NotValue { .. }
            | Self::Branding { .. }
            | Self::Intersection { .. }
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
            Self::NotPerformed { symbol, path } => write!(
                f,
                "{symbol}: `{path}` is an ability, which is performed: `{path}!(...)`"
            ),
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
            Self::Branding { symbol, brand } => write!(
                f,
                "{symbol}: only the module that declares `{brand}` brands a value with it"
            ),
            Self::Intersection { symbol } => {
                write!(f, "{symbol}: `&` puts brands on a type that is not a brand")
            }
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

impl Error {
    /// The errors about a comptime call's arguments and its result.
    fn fmt_comptime(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotComptime { symbol, found } => write!(
                f,
                "{symbol}: a comptime argument must have one possible value, found `{found}`"
            ),
            Self::NoTree { symbol, found } => {
                write!(
                    f,
                    "{symbol}: a comptime function is handed the tree of a type, and {found} has none"
                )
            }
            Self::NotSpliceable { symbol } => {
                write!(f, "{symbol}: a comptime result must be plain data")
            }
            Self::ComptimeFault { function, fault } => {
                write!(f, "`{function}` faulted during compilation: {fault}")
            }
            Self::NotCode(symbol) => {
                write!(f, "{symbol}: the tree the call gave stands for no code")
            }
            Self::Unnameable { symbol, named } => write!(
                f,
                "{symbol}: the code the call gave names `{named}`, which nothing written here could"
            ),
            Self::Reaches { symbol, .. } => write!(
                f,
                "{symbol}: the code the call gave names what cannot be used as a value"
            ),
            Self::NotType(symbol) => {
                write!(f, "{symbol}: the tree the call gave stands for no type")
            }
            Self::ComptimeCycle(symbol) => {
                write!(f, "{symbol}: depends on a comptime call that depends on it")
            }
            _ => Ok(()),
        }
    }
}

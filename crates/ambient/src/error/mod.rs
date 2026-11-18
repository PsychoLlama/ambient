//! Everything a stage can report about a program.

mod display;

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
    /// An ability is named where a value is expected. It is performed.
    NotPerformed { symbol: Symbol, path: Path },
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
    Operands {
        symbol: Symbol,
        /// The operator as it is written.
        operator: String,
        found: String,
    },
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
    /// A comptime argument that is to be a type's tree names no type,
    /// or one that has no tree.
    NoTree { symbol: Symbol, found: String },
    /// The tree a derived type's call gave stands for no type, and
    /// why.
    NotType { symbol: Symbol, why: String },
    /// The tree a comptime call gave as code stands for none: no
    /// expression, function, or definitions. And why.
    NotCode { symbol: Symbol, why: String },
    /// The definitions a generated module's call gave include two of
    /// one name.
    GeneratedTwice { symbol: Symbol, name: String },
    /// Code a comptime call gave names what could not be named where
    /// the call is written.
    Unnameable { symbol: Symbol, named: Symbol },
    /// Code a comptime call gave names definitions the check did not
    /// know of when it began. The engine begins it again with them;
    /// nothing else sees this.
    Reaches {
        symbol: Symbol,
        reached: Vec<Symbol>,
    },
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
    /// A value is branded outside the module that declares the brand.
    Branding { symbol: Symbol, brand: Symbol },
    /// Something that is not a brand is intersected with a type, or a
    /// brand is put on a brand.
    Intersection { symbol: Symbol },
    /// Something that is not a target is used as one.
    NotTarget(Symbol),
    /// A target binds an ability to something that cannot handle it.
    NotHandler { symbol: Symbol, handler: Symbol },
    /// A target extends itself, or one of its handlers needs itself.
    TargetCycle(Symbol),
    /// An extern takes a function whose type has a row: native code
    /// hands what it calls no bindings.
    NativeRow(Symbol),
    /// Two targets a target extends bind one ability differently, and it
    /// does not say which it wants.
    TargetConflict { symbol: Symbol, ability: Symbol },
    /// What a target is built by is not a tool: it does not take an
    /// entry and a target and give the files of a build.
    NotTool(Symbol),
    /// Two targets a target extends are built by different tools, and
    /// it does not name its own.
    ToolConflict(Symbol),
    /// A tool takes a build ability that its package was not granted.
    ToolUngranted { symbol: Symbol, ability: Symbol },
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

impl Error {
    /// The definition the error is in, if it is in one.
    #[must_use]
    pub fn symbol(&self) -> Option<&Symbol> {
        match self {
            Self::UnknownModule(_)
            | Self::UnknownPackage { .. }
            | Self::UnknownImport { .. }
            | Self::PrivateImport { .. }
            | Self::ExportsModule { .. }
            | Self::ExportCycle(_)
            | Self::InternalModule { .. }
            | Self::DuplicateName { .. }
            | Self::Pack(_) => None,
            Self::UnknownSymbol(symbol)
            | Self::GenericAbort(symbol)
            | Self::ComptimeCycle(symbol)
            | Self::NotType { symbol, .. }
            | Self::NotCode { symbol, .. }
            | Self::Unnameable { symbol, .. }
            | Self::GeneratedTwice { symbol, .. }
            | Self::Reaches { symbol, .. }
            | Self::NotTarget(symbol)
            | Self::TargetCycle(symbol)
            | Self::NativeRow(symbol)
            | Self::GenericEntry(symbol)
            | Self::ToolConflict(symbol)
            | Self::NotTool(symbol)
            | Self::ToolUngranted { symbol, .. }
            | Self::SpawnOutsideGroup(symbol)
            | Self::ComptimeAbilities(symbol)
            | Self::EntryAbilities(symbol)
            | Self::Uncontractive(symbol)
            | Self::Nesting(symbol)
            | Self::NotCompilable(symbol)
            | Self::ComptimeFault {
                function: symbol, ..
            }
            | Self::UnknownName { symbol, .. }
            | Self::PrivateName { symbol, .. }
            | Self::NotPerformed { symbol, .. }
            | Self::Arity { symbol, .. }
            | Self::Mismatch { symbol, .. }
            | Self::NotCallable { symbol, .. }
            | Self::NoField { symbol, .. }
            | Self::KeyType { symbol, .. }
            | Self::NoMethod { symbol, .. }
            | Self::Method { symbol, .. }
            | Self::Receiver { symbol, .. }
            | Self::Operands { symbol, .. }
            | Self::NotExhaustive { symbol, .. }
            | Self::UnreachableArm { symbol, .. }
            | Self::TypeArguments { symbol, .. }
            | Self::CannotInfer { symbol, .. }
            | Self::NotComptime { symbol, .. }
            | Self::NoTree { symbol, .. }
            | Self::NotSpliceable { symbol }
            | Self::NotBundle { symbol, .. }
            | Self::DuplicateHandler { symbol, .. }
            | Self::NotValue { symbol, .. }
            | Self::Branding { symbol, .. }
            | Self::Intersection { symbol }
            | Self::NotHandler { symbol, .. }
            | Self::TargetConflict { symbol, .. }
            | Self::Unbound { symbol, .. }
            | Self::SpawnBinding { symbol, .. }
            | Self::Ungranted { symbol, .. }
            | Self::UntypedParameter { symbol, .. }
            | Self::Untestable { symbol, .. }
            | Self::Backend { symbol, .. }
            | Self::Invalid { symbol, .. } => Some(symbol),
        }
    }
}

impl std::error::Error for Error {}

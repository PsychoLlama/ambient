//! Everything a stage can report about a program.

mod display;

use serde::{Deserialize, Serialize};

use crate::pack::Refused;
use crate::surface::Path;
use crate::symbol::{ModuleId, PackageId, Symbol};
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
    /// An intrinsic is named where it is not called, or called with
    /// what it does not take.
    Intrinsic {
        symbol: Symbol,
        path: Path,
        reason: String,
    },
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
    /// A record holds under a contract a value that is not of its
    /// type.
    ContractType {
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
    /// A `??` follows a value that is never null, or with `always`
    /// one that is nothing else.
    NoFallback { symbol: Symbol, always: bool },
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
    /// A constant's value is written as something other than a value
    /// form: it calls, performs, tests, or binds.
    NotConstant(Symbol),
    /// A constant's value holds the constant itself, outside any
    /// function.
    ConstantCycle(Symbol),
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
    /// A native handler says it is the default of an ability another
    /// package declares.
    ForeignDefault { symbol: Symbol, ability: Symbol },
    /// A definition is annotated under a key that is no tag, or with
    /// a value that is not plain data: `key` says which.
    NotAnnotation { symbol: Symbol, key: bool },
    /// What a target is built by, or a build step, is not a tool: it
    /// does not take a `build.Build` and give `build.Files`.
    NotTool(Symbol),
    /// What a manifest lists as a lint is not one: it does not take a
    /// `build.Selection` and give `build.Diagnostics`.
    NotLint(Symbol),
    /// A test that takes something.
    TestTakes(Symbol),
    /// What a manifest lists as a way to find tests is not one: it does
    /// not take a `build.Selection` and give `test.Tests`.
    NotFinder(Symbol),
    /// What a manifest's `[test] run` names is not typed as a runner.
    NotRunner(Symbol),
    /// What a lint found wrong with a definition.
    Lint {
        symbol: Symbol,
        lint: Symbol,
        message: String,
    },
    /// A build under a target was given two files for one path.
    BuiltTwice { symbol: Symbol, path: String },
    /// Two targets a target extends are built by different tools, and
    /// it does not name its own.
    ToolConflict(Symbol),
    /// A tool takes a build ability that its package was not granted.
    /// A program's entry has type parameters.
    GenericEntry(Symbol),
    /// An ability is used where nothing binds it. `closure` is whether
    /// that is in a function written in place, which may take it in
    /// its row.
    Unbound {
        symbol: Symbol,
        ability: Symbol,
        closure: bool,
    },
    /// A program's entry takes an ability that the target it is run
    /// under does not bind.
    Unoffered {
        symbol: Symbol,
        target: Symbol,
        ability: Symbol,
    },
    /// A spawn is written outside any group, or inside a function inside
    /// one.
    SpawnOutsideGroup(Symbol),
    /// A spawned task uses an ability bound inside its group, which may
    /// be over before the task is.
    SpawnBinding { symbol: Symbol, ability: Symbol },
    /// A comptime call takes a build ability that the package it is
    /// written in was not granted.
    Ungranted { symbol: Symbol, ability: Symbol },
    /// A function or an extern takes an ability its package is not
    /// granted. See "Grants" in `docs/spec/packages.md`.
    Takes { symbol: Symbol, ability: Symbol },
    /// A grant in a manifest names no ability and no module.
    UnknownGrant {
        package: PackageId,
        dependency: String,
        grant: String,
    },
    /// A comptime call names a function that takes abilities, and the
    /// build has no target to provide them.
    ComptimeAbilities(Symbol),
    /// A program's entry takes abilities, and nothing provides them.
    EntryAbilities(Symbol),
    /// A lambda parameter has no type, and nothing expects one of it.
    UntypedParameter { symbol: Symbol, name: String },
    /// A recursive type that never reaches a constructor.
    Uncontractive(Symbol),
    /// A type match that names the type definition it is in, however
    /// indirectly.
    MatchesItself(Symbol),
    /// A cast or a pattern tests for a function type that mentions a
    /// type parameter, which no function says it is of.
    Untestable { symbol: Symbol, found: String },
    /// A cast or a pattern tests for a type that holds a type match left
    /// open by a type parameter, which no value says it is of.
    UntestableMatch { symbol: Symbol, found: String },
    /// An arm of a match could tell a field of a record only by asking
    /// a function for its type, which may turn away a value it names.
    AsksAFunction { symbol: Symbol, found: String },
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
            | Self::UnknownGrant { .. }
            | Self::Pack(_) => None,
            Self::UnknownSymbol(symbol)
            | Self::GenericAbort(symbol)
            | Self::ComptimeCycle(symbol)
            | Self::NotConstant(symbol)
            | Self::ConstantCycle(symbol)
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
            | Self::NotLint(symbol)
            | Self::NotFinder(symbol)
            | Self::NotRunner(symbol)
            | Self::TestTakes(symbol)
            | Self::Lint { symbol, .. }
            | Self::BuiltTwice { symbol, .. }
            | Self::NotAnnotation { symbol, .. }
            | Self::SpawnOutsideGroup(symbol)
            | Self::ComptimeAbilities(symbol)
            | Self::EntryAbilities(symbol)
            | Self::Uncontractive(symbol)
            | Self::MatchesItself(symbol)
            | Self::Nesting(symbol)
            | Self::NotCompilable(symbol)
            | Self::ComptimeFault {
                function: symbol, ..
            }
            | Self::UnknownName { symbol, .. }
            | Self::PrivateName { symbol, .. }
            | Self::NotPerformed { symbol, .. }
            | Self::Intrinsic { symbol, .. }
            | Self::Arity { symbol, .. }
            | Self::Mismatch { symbol, .. }
            | Self::NotCallable { symbol, .. }
            | Self::NoField { symbol, .. }
            | Self::ContractType { symbol, .. }
            | Self::NoMethod { symbol, .. }
            | Self::Method { symbol, .. }
            | Self::Receiver { symbol, .. }
            | Self::Operands { symbol, .. }
            | Self::NotExhaustive { symbol, .. }
            | Self::UnreachableArm { symbol, .. }
            | Self::NoFallback { symbol, .. }
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
            | Self::ForeignDefault { symbol, .. }
            | Self::Unbound { symbol, .. }
            | Self::Unoffered { symbol, .. }
            | Self::SpawnBinding { symbol, .. }
            | Self::Ungranted { symbol, .. }
            | Self::Takes { symbol, .. }
            | Self::UntypedParameter { symbol, .. }
            | Self::Untestable { symbol, .. }
            | Self::UntestableMatch { symbol, .. }
            | Self::AsksAFunction { symbol, .. }
            | Self::Backend { symbol, .. }
            | Self::Invalid { symbol, .. } => Some(symbol),
        }
    }
}

impl std::error::Error for Error {}

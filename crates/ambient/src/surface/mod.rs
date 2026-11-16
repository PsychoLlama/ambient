//! Surface trees: what an author writes. See `spec/trees.md`.
//!
//! The trees are generic over how they name things. As authored, a name is a
//! [`Path`]; after resolution it is a [`Reference`]. Nothing else differs
//! between the two, so they share one shape.
//!
//! These types are plain data that anything may read. A [`Module`] can only
//! be made by a [`crate::builder`], which is what rejects ill-formed trees.

mod operator;
mod shape;
pub mod shown;
mod visit;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use operator::Operator;
pub use shape::Shape;
pub use shown::Shown;

use crate::literal::{Brand, Literal};
use crate::symbol::Symbol;

/// A dotted name as written: `x`, `math.add`, `pkg.util.math`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Path(pub Vec<String>);

impl Path {
    /// Splits a dotted name into its segments.
    #[must_use]
    pub fn parse(dotted: &str) -> Self {
        Self(dotted.split('.').map(str::to_owned).collect())
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.join("."))
    }
}

/// What a name means once resolved.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Reference {
    /// A parameter or local binder, numbered in binding order within its
    /// function.
    Local(usize),
    /// A type parameter of the enclosing definition, by position.
    Parameter(usize),
    Definition(Symbol),
}

/// A file of definitions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Module {
    pub(crate) imports: Vec<Import>,
    pub(crate) definitions: Vec<Definition>,
    /// For a library module: the modules of its package that other
    /// packages may reach, by their paths within it.
    pub(crate) exposes: Vec<Path>,
}

impl Module {
    #[must_use]
    pub fn imports(&self) -> &[Import] {
        &self.imports
    }

    #[must_use]
    pub fn definitions(&self) -> &[Definition] {
        &self.definitions
    }

    #[must_use]
    pub fn exposes(&self) -> &[Path] {
        &self.exposes
    }
}

/// Brings one definition or one module into scope under `alias`.
///
/// Whether the path's last segment is a module or a definition is decided by
/// resolution, not by the import.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    pub path: Path,
    pub alias: String,
    /// Whether the module re-exports what it imports here: other
    /// modules then reach the definition through this one too.
    pub public: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Definition<N = Path> {
    pub name: String,
    pub public: bool,
    pub doc: Option<String>,
    pub kind: DefinitionKind<N>,
}

/// The name of the one definition a generated module is written with.
pub const GENERATOR: &str = "__module";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefinitionKind<N = Path> {
    Type(TypeDefinition<N>),
    /// A type a comptime call gives: the call, which is run for the
    /// type's tree. See "Splice positions" in `spec/comptime.md`.
    Derived(Expr<N>),
    /// A function that is whatever tree a comptime call gives: the
    /// call.
    Generated(Expr<N>),
    /// The comptime call that gives everything a generated module
    /// defines. It is the one definition such a module is written with,
    /// under the name [`GENERATOR`].
    Module(Expr<N>),
    Function(Function<N>),
    Ability(Ability<N>),
    Key(KeyDefinition<N>),
    /// A tag: a name for one value, told from every other by where it
    /// is declared. See "Tags" in `spec/definitions.md`.
    Tag,
    /// A brand: a name for a mark on a type, told from every other by
    /// where it is declared. See "Brands" in `spec/types.md`.
    Brand,
    Extern(Extern<N>),
    Target(Target<N>),
}

/// A definition implemented in native code. See `spec/externs.md`.
impl<N> DefinitionKind<N> {
    /// The namespace a definition of this kind is named in. A tag is
    /// named as a type, and through that as a value.
    #[must_use]
    pub fn namespace(&self) -> crate::symbol::Namespace {
        use crate::symbol::Namespace;
        match self {
            Self::Type(_) | Self::Derived(_) | Self::Tag | Self::Brand => Namespace::Type,
            Self::Function(_)
            | Self::Generated(_)
            | Self::Module(_)
            | Self::Extern(_)
            | Self::Target(_) => Namespace::Value,
            Self::Ability(_) => Namespace::Ability,
            Self::Key(_) => Namespace::Key,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extern<N = Path> {
    pub kind: ExternKind<N>,
    /// Whether it may suspend the task that performs it: only a
    /// handler may. See `spec/externs.md`.
    pub suspends: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExternKind<N = Path> {
    /// A function, called like any other, which may perform the
    /// abilities in `row` and is pure but for them.
    Function {
        params: Vec<Param<N>>,
        ret: Type<N>,
        row: Vec<N>,
    },
    /// An implementation of `ability`, which may perform the abilities in
    /// `row`. It can only be bound by a target.
    Handler { ability: N, row: Vec<N> },
}

/// The world a program runs in: a handler for each ability it offers.
/// See `spec/targets.md`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target<N = Path> {
    /// Targets whose bindings this one starts from.
    pub extends: Vec<N>,
    /// An ability, and the function or native handler bound to it.
    pub bindings: Vec<(N, N)>,
}

/// An operation with a signature and no body. See `spec/abilities.md`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ability<N = Path> {
    /// The type parameters of its signature. A handler for it is then
    /// a generic function.
    pub type_params: Vec<TypeParam<N>>,
    pub params: Vec<Param<N>>,
    pub ret: Type<N>,
}

/// A key: what a record holds a value under without reserving a name.
/// See `spec/methods.md`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyDefinition<N = Path> {
    /// The type of what a record holds under the key.
    pub ty: Type<N>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeDefinition<N = Path> {
    /// The names of its type parameters.
    pub params: Vec<String>,
    pub ty: Type<N>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeParam<N = Path> {
    pub name: String,
    /// An upper bound on what the parameter may be instantiated with.
    pub bound: Option<Type<N>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Function<N = Path> {
    pub type_params: Vec<TypeParam<N>>,
    pub params: Vec<Param<N>>,
    pub ret: Type<N>,
    /// The abilities it takes from its caller.
    pub row: Vec<N>,
    pub body: Expr<N>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Param<N = Path> {
    pub name: String,
    pub ty: Type<N>,
}

/// What a record is keyed by.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Key<N = Path> {
    Field(String),
    /// A definition: an ability, for its handler, or a key. A record
    /// keyed only by abilities is a bundle.
    Ability(N),
}

impl<N> Key<N> {
    /// Whether this is the field `name`.
    #[must_use]
    pub fn is_field(&self, name: &str) -> bool {
        matches!(self, Self::Field(field) if field == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Type<N = Path> {
    Never,
    Unknown,
    Null,
    Bool,
    Int,
    Float,
    String,
    Bytes,
    /// Every tag value.
    Tag,
    Literal(Literal),
    /// A brand on its own: the type of a handle minted under it.
    Brand(Brand),
    /// A type and the brands on it. Written, each brand is a name;
    /// normalized, each is a [`Type::Brand`], in order, and the type
    /// under them is not branded.
    Branded(Box<Type<N>>, Vec<Type<N>>),
    Record(Vec<(Key<N>, Type<N>)>),
    /// A method, as a key of a record type holds one: what it needs of
    /// the record it is read from, and the function reading it gives.
    /// See `spec/methods.md`.
    Method {
        needs: Box<Type<N>>,
        function: Box<Type<N>>,
    },
    Tuple(Vec<Type<N>>),
    List(Box<Type<N>>),
    /// A cell that holds a value of the type.
    Cell(Box<Type<N>>),
    /// An iterator of items of the type. See `spec/iteration.md`.
    Iterator(Box<Type<N>>),
    Function {
        params: Vec<Type<N>>,
        ret: Box<Type<N>>,
        /// The abilities a caller must pass.
        row: Vec<N>,
    },
    Union(Vec<Type<N>>),
    /// A type parameter, or a type definition applied to arguments.
    Named(N, Vec<Type<N>>),
    /// A function type with type parameters of its own: the bound of
    /// each, and the function type they are parameters of. See
    /// "Generics" in `spec/types.md`.
    Generic {
        bounds: Vec<Type<N>>,
        function: Box<Type<N>>,
    },
    /// A type parameter of a [`Type::Generic`] this is inside, counted
    /// from the innermost: its parameters in order, then those of the
    /// one around it. A name is not part of a type, so two generic
    /// function types that differ only in names are one type.
    Bound(usize),
}

/// What can be done with a cell. See `spec/cells.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CellOp {
    /// Makes a cell: its type, then its first value.
    New,
    /// The value a cell holds.
    Read,
    /// A cell, the value read, and a new one: puts the new one in if the
    /// cell still holds the value read, and says whether it did.
    Replace,
    /// A cell and the value read: suspends until the cell holds another.
    Wait,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pattern<N = Path> {
    /// Matches anything.
    Wildcard,
    Literal(Literal),
    /// Matches a record that has at least these fields, each matching.
    Record(Vec<(String, Pattern<N>)>),
    /// Matches a tuple of exactly this length, each member matching.
    Tuple(Vec<Pattern<N>>),
    /// Matches a value the type describes.
    Is(Type<N>),
    /// Matches what the inner pattern matches, and binds it to a name.
    Bind(String, Box<Pattern<N>>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arm<N = Path> {
    pub pattern: Pattern<N>,
    pub body: Expr<N>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expr<N = Path> {
    Null,
    /// Where the reader found an expression: a mark of its own, around
    /// what it found there. Only the reader makes one, and it takes
    /// them all out before a module leaves it (`syntax/place.rs`).
    At(usize, Box<Expr<N>>),
    Literal(Literal),
    Name(N),
    /// A generic function with its type arguments written out.
    Instantiate {
        function: N,
        args: Vec<Type<N>>,
    },
    Call {
        callee: Box<Expr<N>>,
        args: Vec<Expr<N>>,
    },
    Record(Vec<(Key<N>, Expr<N>)>),
    /// A function as a method of the record it is a field of: its first
    /// parameter is the record the field is read from. Only a field of
    /// a record is one. See `spec/methods.md`.
    Method(Box<Expr<N>>),
    Tuple(Vec<Expr<N>>),
    List(Vec<Expr<N>>),
    Field {
        target: Box<Expr<N>>,
        key: Key<N>,
    },
    /// A record with the fields of both, `right`'s where both have one.
    Merge {
        left: Box<Expr<N>>,
        right: Box<Expr<N>>,
    },
    /// Binds `name` to `value` for the extent of `body`.
    Let {
        name: String,
        value: Box<Expr<N>>,
        body: Box<Expr<N>>,
    },
    /// An operation of the language on its operands. See
    /// `spec/operations.md`.
    Operation {
        operator: Operator,
        operands: Vec<Expr<N>>,
    },
    /// A method call: `method` of `target`, with `args`. See
    /// `spec/methods.md`. What it means waits for the type of `target`.
    Send {
        target: Box<Expr<N>>,
        method: Key<N>,
        args: Vec<Expr<N>>,
    },
    /// Takes the first arm whose pattern matches the subject.
    Match {
        subject: Box<Expr<N>>,
        arms: Vec<Arm<N>>,
    },
    /// A function written in place. A parameter without a type takes it
    /// from where the lambda is used.
    /// `row` is what it takes from its caller; any other ability it uses
    /// is captured where the lambda is written. `returns` is what it
    /// gives, if that is stated: otherwise it is what the body gives.
    /// With `type_params` it is a generic function, and its type a
    /// generic function type.
    Lambda {
        type_params: Vec<TypeParam<N>>,
        params: Vec<(String, Option<Type<N>>)>,
        row: Vec<N>,
        returns: Option<Type<N>>,
        body: Box<Expr<N>>,
    },
    /// Calls the handler bound to an ability.
    Perform {
        ability: N,
        args: Vec<Expr<N>>,
    },
    /// Evaluates `body`, and then `cleanup`, however `body` ended. Is what
    /// `body` gave. See `spec/cleanup.md`.
    /// Binds a value that carries its own cleanup, as a let does, and
    /// calls what the value holds under the standard library's key
    /// when `body` exits.
    Using {
        name: String,
        value: Box<Expr<N>>,
        body: Box<Expr<N>>,
    },
    /// Evaluates `body` within limits, both integers: a number of steps
    /// and an amount of allocation. Is what it gave, or the limit that ran
    /// out. See `spec/budgets.md`.
    Budget {
        steps: Box<Expr<N>>,
        allocation: Box<Expr<N>>,
        body: Box<Expr<N>>,
    },
    /// Evaluates the body, and is what it gave, or the fault that ended
    /// it, as an error. See `spec/values.md`.
    Boundary(Box<Expr<N>>),
    /// Evaluates the body, and is what it gave once every task it spawned
    /// is over. See `spec/tasks.md`.
    Group(Box<Expr<N>>),
    /// Starts an expression as a task of the group it is written in, and
    /// is null.
    Spawn(Box<Expr<N>>),
    /// Evaluates each expression as a task, and is the tuple of their
    /// results once every one is over.
    All(Vec<Expr<N>>),
    /// Evaluates each expression as a task, is the result of the first
    /// that is over, and cancels the rest.
    First(Vec<Expr<N>>),
    /// An operation on a cell. `ty` is the cell's type, for
    /// [`CellOp::New`].
    Cell {
        operation: CellOp,
        ty: Option<Type<N>>,
        args: Vec<Expr<N>>,
    },
    /// Evaluates `body`, calling `observer` with each ability it performs,
    /// the arguments, and what came back. See `spec/handlers.md`.
    ///
    /// Counts as one binder, numbered after those in `observer`: it holds
    /// the observer.
    Observe {
        observer: Box<Expr<N>>,
        body: Box<Expr<N>>,
    },
    /// What an observer is told an ability is: a string that is equal
    /// only for the same ability.
    AbilityId(N),
    /// Binds the handlers in `bundle` for the extent of `body`. With
    /// `only`, the body sees no other binding.
    ///
    /// A provide counts as one binder, numbered after those in `bundle`:
    /// it holds the bundle while the handlers are read from it.
    Provide {
        bundle: Box<Expr<N>>,
        only: bool,
        body: Box<Expr<N>>,
    },
    /// A call that runs during compilation, replaced by its result.
    Comptime(Box<Expr<N>>),
    /// `value` at type `ty` if the type describes it, and `otherwise` if not.
    Cast {
        value: Box<Expr<N>>,
        ty: Type<N>,
        otherwise: Box<Expr<N>>,
    },
}

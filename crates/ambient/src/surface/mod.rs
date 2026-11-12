//! Surface trees: what an author writes. See `spec/trees.md`.
//!
//! The trees are generic over how they name things. As authored, a name is a
//! [`Path`]; after resolution it is a [`Reference`]. Nothing else differs
//! between the two, so they share one shape.
//!
//! These types are plain data that anything may read. A [`Module`] can only
//! be made by a [`crate::builder`], which is what rejects ill-formed trees.

mod visit;

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::literal::Literal;
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
}

/// Brings one definition or one module into scope under `alias`.
///
/// Whether the path's last segment is a module or a definition is decided by
/// resolution, not by the import.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    pub path: Path,
    pub alias: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Definition<N = Path> {
    pub name: String,
    pub public: bool,
    pub doc: Option<String>,
    pub kind: DefinitionKind<N>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefinitionKind<N = Path> {
    Type(TypeDefinition<N>),
    Function(Function<N>),
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
    pub body: Expr<N>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Param<N = Path> {
    pub name: String,
    pub ty: Type<N>,
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
    Literal(Literal),
    Record(Vec<(String, Type<N>)>),
    Tuple(Vec<Type<N>>),
    List(Box<Type<N>>),
    Function {
        params: Vec<Type<N>>,
        ret: Box<Type<N>>,
    },
    Union(Vec<Type<N>>),
    /// A type parameter, or a type definition applied to arguments.
    Named(N, Vec<Type<N>>),
}

/// An operator. Which primitive operation it means depends on its operands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operator {
    Add,
    Subtract,
    Multiply,
    Less,
    Equal,
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
    Record(Vec<(String, Expr<N>)>),
    Tuple(Vec<Expr<N>>),
    List(Vec<Expr<N>>),
    Field {
        target: Box<Expr<N>>,
        name: String,
    },
    /// Binds `name` to `value` for the extent of `body`.
    Let {
        name: String,
        value: Box<Expr<N>>,
        body: Box<Expr<N>>,
    },
    Binary {
        operator: Operator,
        left: Box<Expr<N>>,
        right: Box<Expr<N>>,
    },
    /// Takes the first arm whose pattern matches the subject.
    Match {
        subject: Box<Expr<N>>,
        arms: Vec<Arm<N>>,
    },
    /// A function written in place. A parameter without a type takes it
    /// from where the lambda is used.
    Lambda {
        params: Vec<(String, Option<Type<N>>)>,
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

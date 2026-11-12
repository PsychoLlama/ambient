//! Surface trees: what an author writes. See `spec/trees.md`.
//!
//! The trees are generic over how they name things. As authored, a name is a
//! [`Path`]; after resolution it is a [`Reference`]. Nothing else differs
//! between the two, so they share one shape.
//!
//! These types are plain data that anything may read. A [`Module`] can only
//! be made by a [`crate::builder`], which is what rejects ill-formed trees.

use std::fmt;

use serde::{Deserialize, Serialize};

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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reference {
    /// A parameter or local binder, numbered in binding order within its
    /// function.
    Local(usize),
    Definition(Symbol),
}

/// A file of definitions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Definition<N = Path> {
    pub name: String,
    pub public: bool,
    pub doc: Option<String>,
    pub kind: DefinitionKind<N>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DefinitionKind<N = Path> {
    Type(Type<N>),
    Function(Function<N>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Function<N = Path> {
    pub params: Vec<Param<N>>,
    pub ret: Type<N>,
    pub body: Expr<N>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Param<N = Path> {
    pub name: String,
    pub ty: Type<N>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Type<N = Path> {
    Null,
    Bool,
    Int,
    Float,
    String,
    Record(Vec<(String, Type<N>)>),
    Function {
        params: Vec<Type<N>>,
        ret: Box<Type<N>>,
    },
    Named(N),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Expr<N = Path> {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Name(N),
    Call {
        callee: Box<Expr<N>>,
        args: Vec<Expr<N>>,
    },
    Record(Vec<(String, Expr<N>)>),
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
}

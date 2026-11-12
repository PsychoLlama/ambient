//! Core trees: the checked form of a definition. See `spec/core.md`.
//!
//! TODO: only the fragment the checker handles so far is here. The node set
//! in the spec is the target.

mod encode;

use serde::{Deserialize, Serialize};

use crate::literal::Literal;
use crate::symbol::Symbol;

pub(crate) use encode::encode_signature;

/// A reference to another definition, as an index into the slot table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Slot(pub usize);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Type {
    Never,
    Unknown,
    Null,
    Bool,
    Int,
    Float,
    String,
    Literal(Literal),
    /// Fields are ordered by name.
    Record(Vec<(String, Type)>),
    Tuple(Vec<Type>),
    List(Box<Type>),
    Function {
        params: Vec<Type>,
        ret: Box<Type>,
    },
    /// Members are ordered by their encodings, without duplicates.
    Union(Vec<Type>),
    /// A type parameter of the enclosing definition, by position.
    Parameter(usize),
    /// A recursive type definition applied to arguments. A non-recursive
    /// one never appears: it is replaced by its expansion.
    Reference(Slot, Vec<Type>),
}

/// An operation the language defines. A primitive has no slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Primitive {
    IntAdd,
    IntSubtract,
    IntMultiply,
    IntLess,
    FloatAdd,
    FloatSubtract,
    FloatMultiply,
    FloatLess,
    /// Structural equality, over every shape.
    Equal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pattern {
    Wildcard,
    Literal(Literal),
    Record(Vec<(String, Pattern)>),
    Tuple(Vec<Pattern>),
    Is(Type),
    /// Binds the next local to what the inner pattern matches. The local
    /// is numbered before any the inner pattern binds.
    Bind(Box<Pattern>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expr {
    Null,
    Literal(Literal),
    Local(usize),
    /// A value the enclosing lambda captured, by position.
    Captured(usize),
    /// A function, with an argument for each of its type parameters.
    Reference(Slot, Vec<Type>),
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    /// Fields keep the order they were written in: it is their evaluation
    /// order.
    Record(Vec<(String, Expr)>),
    Tuple(Vec<Expr>),
    List(Vec<Expr>),
    Field {
        target: Box<Expr>,
        name: String,
    },
    /// Binds the next local to `value` for the extent of `body`.
    Let {
        value: Box<Expr>,
        body: Box<Expr>,
    },
    Primitive {
        operation: Primitive,
        args: Vec<Expr>,
    },
    /// Takes the first arm whose pattern matches. Checking has shown that
    /// one always does.
    Match {
        subject: Box<Expr>,
        arms: Vec<(Pattern, Expr)>,
    },
    Cast {
        value: Box<Expr>,
        ty: Type,
        otherwise: Box<Expr>,
    },
    /// A function value: a lambda's definition and the values it captures.
    Closure {
        slot: Slot,
        type_args: Vec<Type>,
        captures: Vec<Expr>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Function {
    /// The bound of each type parameter.
    pub type_params: Vec<Type>,
    /// The type of each captured value. Empty unless this is a lambda.
    pub captures: Vec<Type>,
    pub params: Vec<Type>,
    pub ret: Type,
    pub body: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tree {
    Type {
        /// How many type parameters the definition takes.
        params: usize,
        ty: Type,
    },
    Function(Function),
}

/// A core definition whose slots are still symbols: what checking yields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Open {
    pub tree: Tree,
    /// What each slot refers to, in order of first use.
    pub slots: Vec<Symbol>,
    /// The lambdas written in this definition, each a definition of its
    /// own. A slot reaches one through its symbol.
    pub lambdas: Vec<Open>,
}

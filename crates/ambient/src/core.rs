//! Core trees: the checked form of a definition. See `spec/core.md`.
//!
//! TODO: only the fragment the checker handles so far is here. The node set
//! in the spec is the target.

use serde::{Deserialize, Serialize};

use crate::literal::Literal;
use crate::symbol::Symbol;

/// The version named by every encoding. Changing the node set or the
/// encoding means changing this, which changes every hash.
const FORMAT: u8 = 0;

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
    /// A recursive type definition. A non-recursive one never appears: it
    /// is replaced by its expansion.
    Reference(Slot),
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
    Reference(Slot),
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Function {
    pub params: Vec<Type>,
    pub ret: Type,
    pub body: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tree {
    Type(Type),
    Function(Function),
}

/// A core definition whose slots are still symbols: what checking yields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Open {
    pub tree: Tree,
    /// What each slot refers to, in order of first use.
    pub slots: Vec<Symbol>,
}

impl Tree {
    /// The canonical encoding: the bytes a hash is computed over.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![FORMAT];
        match self {
            Self::Type(ty) => {
                out.push(0);
                encode_type(ty, &mut out);
            }
            Self::Function(function) => {
                out.push(1);
                encode_types(&function.params, &mut out);
                encode_type(&function.ret, &mut out);
                encode_expr(&function.body, &mut out);
            }
        }
        out
    }
}

impl Type {
    /// The canonical encoding of the type alone.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        encode_type(self, &mut out);
        out
    }
}

/// The canonical encoding of a signature alone, for interface digests.
pub(crate) fn encode_signature(params: &[Type], ret: &Type) -> Vec<u8> {
    let mut out = vec![FORMAT];
    encode_types(params, &mut out);
    encode_type(ret, &mut out);
    out
}

fn encode_types(types: &[Type], out: &mut Vec<u8>) {
    unsigned(types.len() as u64, out);
    for ty in types {
        encode_type(ty, out);
    }
}

fn encode_literal(literal: &Literal, out: &mut Vec<u8>) {
    match literal {
        Literal::Bool(value) => out.extend([0, u8::from(*value)]),
        Literal::Int(value) => {
            out.push(1);
            out.extend_from_slice(&value.to_le_bytes());
        }
        Literal::Float(bits) => {
            out.push(2);
            out.extend_from_slice(&bits.to_le_bytes());
        }
        Literal::String(value) => {
            out.push(3);
            text(value, out);
        }
    }
}

fn encode_type(ty: &Type, out: &mut Vec<u8>) {
    match ty {
        Type::Never => out.push(0),
        Type::Unknown => out.push(1),
        Type::Null => out.push(2),
        Type::Bool => out.push(3),
        Type::Int => out.push(4),
        Type::Float => out.push(5),
        Type::String => out.push(6),
        Type::Literal(literal) => {
            out.push(7);
            encode_literal(literal, out);
        }
        Type::Record(fields) => {
            out.push(8);
            unsigned(fields.len() as u64, out);
            for (name, ty) in fields {
                text(name, out);
                encode_type(ty, out);
            }
        }
        Type::Tuple(members) => {
            out.push(9);
            encode_types(members, out);
        }
        Type::List(member) => {
            out.push(10);
            encode_type(member, out);
        }
        Type::Function { params, ret } => {
            out.push(11);
            encode_types(params, out);
            encode_type(ret, out);
        }
        Type::Union(members) => {
            out.push(12);
            encode_types(members, out);
        }
        Type::Reference(slot) => {
            out.push(13);
            unsigned(slot.0 as u64, out);
        }
    }
}

fn encode_pattern(pattern: &Pattern, out: &mut Vec<u8>) {
    match pattern {
        Pattern::Wildcard => out.push(0),
        Pattern::Literal(literal) => {
            out.push(1);
            encode_literal(literal, out);
        }
        Pattern::Record(fields) => {
            out.push(2);
            unsigned(fields.len() as u64, out);
            for (name, pattern) in fields {
                text(name, out);
                encode_pattern(pattern, out);
            }
        }
        Pattern::Tuple(members) => {
            out.push(3);
            unsigned(members.len() as u64, out);
            for member in members {
                encode_pattern(member, out);
            }
        }
        Pattern::Is(ty) => {
            out.push(4);
            encode_type(ty, out);
        }
        Pattern::Bind(inner) => {
            out.push(5);
            encode_pattern(inner, out);
        }
    }
}

fn encode_exprs(exprs: &[Expr], out: &mut Vec<u8>) {
    unsigned(exprs.len() as u64, out);
    for expr in exprs {
        encode_expr(expr, out);
    }
}

fn encode_expr(expr: &Expr, out: &mut Vec<u8>) {
    match expr {
        Expr::Null => out.push(0),
        Expr::Literal(literal) => {
            out.push(1);
            encode_literal(literal, out);
        }
        Expr::Local(index) => {
            out.push(2);
            unsigned(*index as u64, out);
        }
        Expr::Reference(slot) => {
            out.push(3);
            unsigned(slot.0 as u64, out);
        }
        Expr::Call { callee, args } => {
            out.push(4);
            encode_expr(callee, out);
            encode_exprs(args, out);
        }
        Expr::Record(fields) => {
            out.push(5);
            unsigned(fields.len() as u64, out);
            for (name, value) in fields {
                text(name, out);
                encode_expr(value, out);
            }
        }
        Expr::Tuple(members) => {
            out.push(6);
            encode_exprs(members, out);
        }
        Expr::List(members) => {
            out.push(7);
            encode_exprs(members, out);
        }
        Expr::Field { target, name } => {
            out.push(8);
            encode_expr(target, out);
            text(name, out);
        }
        Expr::Let { value, body } => {
            out.push(9);
            encode_expr(value, out);
            encode_expr(body, out);
        }
        Expr::Primitive { operation, args } => {
            out.push(10);
            out.push(*operation as u8);
            encode_exprs(args, out);
        }
        Expr::Match { subject, arms } => {
            out.push(11);
            encode_expr(subject, out);
            unsigned(arms.len() as u64, out);
            for (pattern, body) in arms {
                encode_pattern(pattern, out);
                encode_expr(body, out);
            }
        }
        Expr::Cast {
            value,
            ty,
            otherwise,
        } => {
            out.push(12);
            encode_expr(value, out);
            encode_type(ty, out);
            encode_expr(otherwise, out);
        }
    }
}

/// LEB128.
fn unsigned(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn text(value: &str, out: &mut Vec<u8>) {
    unsigned(value.len() as u64, out);
    out.extend_from_slice(value.as_bytes());
}

//! Core trees: the checked form of a definition. See `spec/core.md`.
//!
//! TODO: only the fragment the checker handles so far is here. The node set
//! in the spec is the target.

use serde::{Deserialize, Serialize};

use crate::symbol::Symbol;

/// The version named by every encoding. Changing the node set or the
/// encoding means changing this, which changes every hash.
const FORMAT: u8 = 0;

/// A reference to another definition, as an index into the slot table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Slot(pub usize);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Type {
    Null,
    Bool,
    Int,
    Float,
    String,
    /// Fields are ordered by name.
    Record(Vec<(String, Type)>),
    Function {
        params: Vec<Type>,
        ret: Box<Type>,
    },
    Reference(Slot),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Local(usize),
    Reference(Slot),
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    /// Fields keep the order they were written in: it is their evaluation
    /// order.
    Record(Vec<(String, Expr)>),
    Field {
        target: Box<Expr>,
        name: String,
    },
    /// Binds the next local to `value` for the extent of `body`.
    Let {
        value: Box<Expr>,
        body: Box<Expr>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Function {
    pub params: Vec<Type>,
    pub ret: Type,
    pub body: Expr,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Tree {
    Type(Type),
    Function(Function),
}

/// A core definition whose slots are still symbols: what checking yields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
                encode_function(function, &mut out);
            }
        }
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

fn encode_function(function: &Function, out: &mut Vec<u8>) {
    encode_types(&function.params, out);
    encode_type(&function.ret, out);
    encode_expr(&function.body, out);
}

fn encode_types(types: &[Type], out: &mut Vec<u8>) {
    unsigned(types.len() as u64, out);
    for ty in types {
        encode_type(ty, out);
    }
}

fn encode_type(ty: &Type, out: &mut Vec<u8>) {
    match ty {
        Type::Null => out.push(0),
        Type::Bool => out.push(1),
        Type::Int => out.push(2),
        Type::Float => out.push(3),
        Type::String => out.push(4),
        Type::Record(fields) => {
            out.push(5);
            unsigned(fields.len() as u64, out);
            for (name, ty) in fields {
                text(name, out);
                encode_type(ty, out);
            }
        }
        Type::Function { params, ret } => {
            out.push(6);
            encode_types(params, out);
            encode_type(ret, out);
        }
        Type::Reference(slot) => {
            out.push(7);
            unsigned(slot.0 as u64, out);
        }
    }
}

fn encode_expr(expr: &Expr, out: &mut Vec<u8>) {
    match expr {
        Expr::Null => out.push(0),
        Expr::Bool(value) => {
            out.push(1);
            out.push(u8::from(*value));
        }
        Expr::Int(value) => {
            out.push(2);
            out.extend_from_slice(&value.to_le_bytes());
        }
        Expr::Float(value) => {
            out.push(3);
            out.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        Expr::String(value) => {
            out.push(4);
            text(value, out);
        }
        Expr::Local(index) => {
            out.push(5);
            unsigned(*index as u64, out);
        }
        Expr::Reference(slot) => {
            out.push(6);
            unsigned(slot.0 as u64, out);
        }
        Expr::Call { callee, args } => {
            out.push(7);
            encode_expr(callee, out);
            unsigned(args.len() as u64, out);
            for arg in args {
                encode_expr(arg, out);
            }
        }
        Expr::Record(fields) => {
            out.push(8);
            unsigned(fields.len() as u64, out);
            for (name, value) in fields {
                text(name, out);
                encode_expr(value, out);
            }
        }
        Expr::Field { target, name } => {
            out.push(9);
            encode_expr(target, out);
            text(name, out);
        }
        Expr::Let { value, body } => {
            out.push(10);
            encode_expr(value, out);
            encode_expr(body, out);
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

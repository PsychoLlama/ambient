//! The canonical encoding: the bytes every hash is computed over.

use super::{Expr, Pattern, Tree, Type};
use crate::literal::Literal;

/// The version named by every encoding. Changing the node set or the
/// encoding means changing this, which changes every hash.
const FORMAT: u8 = 0;

impl Tree {
    /// The canonical encoding: the bytes a hash is computed over.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![FORMAT];
        match self {
            Self::Type { params, ty } => {
                out.push(0);
                unsigned(*params as u64, &mut out);
                encode_type(ty, &mut out);
            }
            Self::Function(function) => {
                out.push(1);
                encode_types(&function.type_params, &mut out);
                encode_types(&function.captures, &mut out);
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
pub(crate) fn encode_signature(type_params: &[Type], params: &[Type], ret: &Type) -> Vec<u8> {
    let mut out = vec![FORMAT];
    encode_types(type_params, &mut out);
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
        Type::Reference(slot, args) => {
            out.push(13);
            unsigned(slot.0 as u64, out);
            encode_types(args, out);
        }
        Type::Parameter(index) => {
            out.push(14);
            unsigned(*index as u64, out);
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
        Expr::Reference(slot, args) => {
            out.push(3);
            unsigned(slot.0 as u64, out);
            encode_types(args, out);
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
        Expr::Captured(index) => {
            out.push(13);
            unsigned(*index as u64, out);
        }
        Expr::Closure {
            slot,
            type_args,
            captures,
        } => {
            out.push(14);
            unsigned(slot.0 as u64, out);
            encode_types(type_args, out);
            encode_exprs(captures, out);
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

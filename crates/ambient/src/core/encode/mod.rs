//! The canonical encoding: the bytes every hash is computed over.
//!
//! A tree is encoded in one of two ways. As it is stored, a slot is its
//! index and a union's members keep their order: that is what a tree is
//! compared and compiled by. Resolved, a slot is whatever the caller says
//! it refers to, and a union's members are ordered by their own resolved
//! encodings: that is what a hash is computed over, so neither slot
//! numbers nor the stored order of a union, which names decide, reach it.

mod expr;

use super::{Slot, Tree, Type};
use crate::literal::Literal;
use expr::{encode_expr, encode_key};

/// Writes what a slot refers to.
pub type Resolve<'a> = &'a dyn Fn(Slot, &mut Vec<u8>);

/// The bytes written so far, and how a slot is written.
struct Sink<'a> {
    bytes: Vec<u8>,
    resolve: Option<Resolve<'a>>,
}

impl<'a> Sink<'a> {
    fn new(resolve: Option<Resolve<'a>>) -> Self {
        Self {
            bytes: Vec::new(),
            resolve,
        }
    }

    fn push(&mut self, byte: u8) {
        self.bytes.push(byte);
    }

    fn extend_from_slice(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    fn slot(&mut self, slot: Slot) {
        match self.resolve {
            Some(resolve) => resolve(slot, &mut self.bytes),
            None => unsigned(slot.0 as u64, self),
        }
    }

    /// The members of a union. Resolved, they are put in the order of
    /// their encodings, which is the one order that names had no part in.
    fn union(&mut self, members: &[Type]) {
        unsigned(members.len() as u64, self);
        if self.resolve.is_none() {
            for member in members {
                encode_type(member, self);
            }
            return;
        }
        let mut encodings: Vec<Vec<u8>> = members
            .iter()
            .map(|member| {
                let mut sink = Sink::new(self.resolve);
                encode_type(member, &mut sink);
                sink.bytes
            })
            .collect();
        encodings.sort();
        for encoding in encodings {
            self.extend_from_slice(&encoding);
        }
    }
}

/// The version named by every encoding. Changing the node set or the
/// encoding means changing this, which changes every hash.
const FORMAT: u8 = 22;

impl Tree {
    /// The encoding of the tree as it is stored: slots by index.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        self.write(Sink::new(None))
    }

    /// The encoding a hash is computed over: each slot as `resolve` writes
    /// it, which must be by content.
    #[must_use]
    pub fn resolved(&self, resolve: Resolve<'_>) -> Vec<u8> {
        self.write(Sink::new(Some(resolve)))
    }

    fn write(&self, mut sink: Sink<'_>) -> Vec<u8> {
        let out = &mut sink;
        out.push(FORMAT);
        match self {
            Self::Type { params, ty } => {
                out.push(0);
                unsigned(*params as u64, out);
                encode_type(ty, out);
            }
            Self::Function(function) => {
                out.push(1);
                encode_types(&function.type_params, out);
                encode_types(&function.captures, out);
                encode_slots(&function.bindings, out);
                encode_types(&function.params, out);
                encode_type(&function.ret, out);
                encode_slots(&function.row, out);
                encode_expr(&function.body, out);
            }
            Self::Ability {
                package,
                tag,
                params,
                ret,
            } => {
                out.push(2);
                text(package, out);
                text(tag, out);
                encode_types(params, out);
                encode_type(ret, out);
            }
            Self::Key { package, tag, ty } => {
                out.push(5);
                text(package, out);
                text(tag, out);
                encode_type(ty, out);
            }
            Self::Extern {
                package,
                tag,
                params,
                ret,
                handles,
                row,
            } => {
                out.push(3);
                text(package, out);
                text(tag, out);
                encode_types(params, out);
                encode_type(ret, out);
                encode_slots(handles.as_slice(), out);
                encode_slots(row, out);
            }
            Self::Target { bindings } => {
                out.push(4);
                unsigned(bindings.len() as u64, out);
                for (ability, handler) in bindings {
                    encode_slots(&[*ability, *handler], out);
                }
            }
        }
        sink.bytes
    }
}

impl Type {
    /// The canonical encoding of the type alone.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Sink::new(None);
        encode_type(self, &mut out);
        out.bytes
    }
}

/// The canonical encoding of a signature alone, for interface digests.
pub(crate) fn encode_signature(
    type_params: &[Type],
    params: &[Type],
    ret: &Type,
    row: &[Slot],
) -> Vec<u8> {
    let mut sink = Sink::new(None);
    let out = &mut sink;
    out.push(FORMAT);
    encode_types(type_params, out);
    encode_types(params, out);
    encode_type(ret, out);
    encode_slots(row, out);
    sink.bytes
}

fn encode_slots(slots: &[Slot], out: &mut Sink<'_>) {
    unsigned(slots.len() as u64, out);
    for slot in slots {
        out.slot(*slot);
    }
}

fn encode_types(types: &[Type], out: &mut Sink<'_>) {
    unsigned(types.len() as u64, out);
    for ty in types {
        encode_type(ty, out);
    }
}

fn encode_literal(literal: &Literal, out: &mut Sink<'_>) {
    match literal {
        Literal::Bool(value) => out.extend_from_slice(&[0, u8::from(*value)]),
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
        Literal::Tag { package, tag } => {
            out.push(4);
            text(package, out);
            text(tag, out);
        }
    }
}

fn encode_type(ty: &Type, out: &mut Sink<'_>) {
    match ty {
        Type::Never => out.push(0),
        Type::Unknown => out.push(1),
        Type::Null => out.push(2),
        Type::Bool => out.push(3),
        Type::Int => out.push(4),
        Type::Float => out.push(5),
        Type::String => out.push(6),
        Type::Tag => out.push(16),
        Type::Literal(literal) => {
            out.push(7);
            encode_literal(literal, out);
        }
        Type::Record(fields) => {
            out.push(8);
            unsigned(fields.len() as u64, out);
            for (key, ty) in fields {
                encode_key(key, out);
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
        Type::Cell(member) => {
            out.push(15);
            encode_type(member, out);
        }
        Type::Iterator(member) => {
            out.push(18);
            encode_type(member, out);
        }
        Type::Generic { bounds, function } => {
            out.push(19);
            encode_types(bounds, out);
            encode_type(function, out);
        }
        Type::Bound(index) => {
            out.push(20);
            unsigned(*index as u64, out);
        }
        Type::Method { needs, function } => {
            out.push(17);
            encode_type(needs, out);
            encode_type(function, out);
        }
        Type::Function { params, ret, row } => {
            out.push(11);
            encode_types(params, out);
            encode_type(ret, out);
            encode_slots(row, out);
        }
        Type::Union(members) => {
            out.push(12);
            out.union(members);
        }
        Type::Reference(slot, args) => {
            out.push(13);
            out.slot(*slot);
            encode_types(args, out);
        }
        Type::Parameter(index) => {
            out.push(14);
            unsigned(*index as u64, out);
        }
    }
}

/// LEB128.
fn unsigned(mut value: u64, out: &mut Sink<'_>) {
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

fn text(value: &str, out: &mut Sink<'_>) {
    unsigned(value.len() as u64, out);
    out.extend_from_slice(value.as_bytes());
}

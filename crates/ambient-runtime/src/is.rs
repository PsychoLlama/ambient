//! Type tests: whether a descriptor describes a value.

use crate::object::{
    KIND_FLOAT, KIND_INT, KIND_LIST, KIND_RECORD, KIND_STRING, KIND_TAG, KIND_TUPLE, boolean,
    bytes, int, kind, payload, words,
};
use crate::record::find;
use crate::{FALSE, NULL, TRUE, Word};

/// The layout of a type descriptor: what a cast tests a value against.
///
/// A descriptor is a node: a tag word, then what the tag says follows. A
/// node refers to another by address. Descriptors are laid out when a
/// program is loaded and never reclaimed.
pub mod descriptor {
    pub const NEVER: u64 = 0;
    pub const UNKNOWN: u64 = 1;
    pub const NULL: u64 = 2;
    pub const BOOL: u64 = 3;
    pub const INT: u64 = 4;
    pub const FLOAT: u64 = 5;
    pub const STRING: u64 = 6;
    /// Then which kind of literal, and its payload: `0` or `1`, an integer,
    /// a float's bits, or a string.
    pub const LITERAL: u64 = 7;
    pub const LITERAL_BOOL: u64 = 0;
    pub const LITERAL_INT: u64 = 1;
    pub const LITERAL_FLOAT: u64 = 2;
    pub const LITERAL_STRING: u64 = 3;
    /// A tag: equal tags are one object.
    pub const LITERAL_TAG: u64 = 4;
    /// Then a count, and that many pairs of a key and a node.
    pub const RECORD: u64 = 8;
    /// Then a count, and that many nodes.
    pub const TUPLE: u64 = 9;
    /// Then the member's node.
    pub const LIST: u64 = 10;
    pub const FUNCTION: u64 = 11;
    /// Then a count, and that many nodes.
    pub const UNION: u64 = 12;
    /// A generic definition at arguments: then the definition's node, a
    /// count, and that many argument nodes.
    pub const APPLY: u64 = 13;
    /// Then the position of a parameter of the definition being applied.
    pub const PARAMETER: u64 = 14;
    /// Then the node this one stands for.
    pub const ALIAS: u64 = 15;
    pub const TAG: u64 = 16;
}

/// The arguments a generic type definition is being read at.
pub(crate) struct Arguments<'a> {
    nodes: &'a [u64],
    /// What the arguments themselves are read at.
    outer: Option<&'a Arguments<'a>>,
}

/// Whether `ty`, a descriptor, describes a value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_is(value: Word, ty: u64) -> Word {
    // An entry point takes words; the descriptor's address arrives as one.
    unsafe { boolean(is(value, ty as usize as *const u64, None)) }
}

// TODO: this recurses as deep as the value nests.
pub(crate) unsafe fn is(value: Word, ty: *const u64, arguments: Option<&Arguments>) -> bool {
    use descriptor as d;
    unsafe {
        let node = |index: usize| ty.add(index).read();
        let child = |index: usize| node(index) as usize as *const u64;
        let nodes =
            |start: usize, count: u64| std::slice::from_raw_parts(ty.add(start), count as usize);
        let each = |children: &[u64], members: &[u64]| {
            children.len() == members.len()
                && children
                    .iter()
                    .zip(members)
                    .all(|(child, member)| is(*member, *child as usize as *const u64, arguments))
        };
        let integer = value & 1 == 1 || kind(value) == KIND_INT;
        match node(0) {
            d::UNKNOWN => true,
            d::NULL => value == NULL,
            d::BOOL => value == TRUE || value == FALSE,
            d::INT => integer,
            d::FLOAT => kind(value) == KIND_FLOAT,
            d::STRING => kind(value) == KIND_STRING,
            d::TAG => kind(value) == KIND_TAG,
            d::LITERAL => match node(1) {
                d::LITERAL_BOOL => value == boolean(node(2) != 0),
                d::LITERAL_INT => integer && int(value) == node(2) as i64,
                d::LITERAL_FLOAT => kind(value) == KIND_FLOAT && payload(value).read() == node(2),
                d::LITERAL_TAG => value == node(2),
                _ => kind(value) == KIND_STRING && bytes(value) == bytes(node(2)),
            },
            d::RECORD => {
                kind(value) == KIND_RECORD
                    && nodes(2, node(1) * 2).chunks(2).all(|field| {
                        find(value, field[0]).is_some_and(|member| {
                            is(member, field[1] as usize as *const u64, arguments)
                        })
                    })
            }
            d::TUPLE => kind(value) == KIND_TUPLE && each(nodes(2, node(1)), words(value)),
            // A tuple is a list of its members.
            d::LIST => {
                matches!(kind(value), KIND_LIST | KIND_TUPLE)
                    && words(value)
                        .iter()
                        .all(|member| is(*member, child(1), arguments))
            }
            d::UNION => nodes(2, node(1))
                .iter()
                .any(|member| is(value, *member as usize as *const u64, arguments)),
            d::APPLY => {
                let applied = Arguments {
                    nodes: nodes(3, node(2)),
                    outer: arguments,
                };
                is(value, child(1), Some(&applied))
            }
            d::PARAMETER => arguments.is_some_and(|arguments| {
                let argument = arguments.nodes[node(1) as usize] as usize as *const u64;
                is(value, argument, arguments.outer)
            }),
            d::ALIAS => is(value, child(1), arguments),
            // TODO: a function value carries no signature yet, so nothing
            // is known to be a function of a given type.
            _ => false,
        }
    }
}

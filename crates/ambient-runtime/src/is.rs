//! Type tests: whether a descriptor describes a value.

use alloc::vec::Vec;

use crate::cell::made_at;
use crate::closure::{described, generic, type_name};
use crate::iterator::{Tested, tested};
use crate::list::members;
use crate::object::{
    KIND_BYTES, KIND_CELL, KIND_CLOSURE, KIND_FLOAT, KIND_HANDLE, KIND_INT, KIND_ITERATOR,
    KIND_LIST, KIND_METHOD, KIND_RECORD, KIND_STRING, KIND_TAG, KIND_TUPLE, boolean, bytes, int,
    kind, payload, words,
};
use crate::record::find;
use crate::same::Types;
use crate::scope::{At, Env, Scope, argument};
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
    pub const LITERAL_BYTES: u64 = 5;
    /// Then a count, and that many pairs of a key and a node.
    pub const RECORD: u64 = 8;
    /// Then a count, and that many nodes.
    pub const TUPLE: u64 = 9;
    /// Then the member's node.
    pub const LIST: u64 = 10;
    /// Then a count and that many parameter nodes, the result's node, a
    /// count, that many keys: the abilities it performs, and the name of
    /// the type as a definition carries it, or zero if the type mentions
    /// a type parameter.
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
    /// Then the node of what the cell holds.
    pub const CELL: u64 = 17;
    /// Then the node of what the method needs of its receiver, and the
    /// node of the function reading it gives. No value is tested
    /// against one: it is only ever compared, inside a cell's type.
    pub const METHOD: u64 = 18;
    /// Then the node of the items, and the name of the type of a
    /// function that steps an iterator of them, as a definition carries
    /// it, or zero if the items' type mentions a type parameter.
    pub const ITERATOR: u64 = 19;
    /// A function type with type parameters of its own: then a count
    /// and that many nodes, the bound of each; the node of the
    /// function type; and the name of the whole type, as a definition
    /// carries it, or zero if it mentions a type parameter of the
    /// definition it is written in.
    pub const GENERIC: u64 = 20;
    /// Then which parameter of the generic function types it is inside,
    /// counted from the innermost. No value is tested against one.
    pub const BOUND: u64 = 21;
    pub const BYTES: u64 = 22;
    /// Then the brand a handle was minted under, a tag.
    pub const HANDLE: u64 = 23;
    /// A function of whatever type: the kind of the value is all that
    /// is tested.
    pub const CALLABLE: u64 = 24;
}

/// The tests a descriptor makes of the values inside a value.
enum Inside {
    /// A record's fields: pairs of a key and a node.
    Fields {
        record: Word,
        fields: &'static [u64],
    },
    /// A tuple's members, each against its own node.
    Members {
        nodes: &'static [u64],
        members: &'static [u64],
    },
    /// A list's members, all against one node.
    Every { node: u64, members: &'static [u64] },
    /// One value against each member of a union.
    Union { value: Word, nodes: &'static [u64] },
}

/// Tests under way: those of one descriptor, and how far along they are.
struct Frame {
    inside: Inside,
    next: usize,
    /// What the tests' nodes are read in.
    scope: Env,
    /// How many scopes had been entered when this began. Those entered
    /// since belong to a test that is over.
    entered: usize,
}

impl Frame {
    /// The answer that settles the frame: one member of a union that
    /// holds, or one of anything else that does not.
    fn settled_by(&self) -> bool {
        matches!(self.inside, Inside::Union { .. })
    }

    unsafe fn advance(&mut self) -> Next {
        let index = self.next;
        self.next += 1;
        let test = |member: Option<&u64>, node: Option<&u64>| match (member, node) {
            (Some(member), Some(node)) => Next::Test(*member, *node),
            _ => Next::Done,
        };
        match &self.inside {
            Inside::Fields { record, fields } => match fields.chunks(2).nth(index) {
                Some(field) => match unsafe { find(*record, field[0]) } {
                    Some(member) => Next::Test(member, field[1]),
                    None => Next::Missing,
                },
                None => Next::Done,
            },
            Inside::Members { nodes, members } => test(members.get(index), nodes.get(index)),
            Inside::Every { node, members } => test(members.get(index), Some(node)),
            Inside::Union { value, nodes } => test(Some(value), nodes.get(index)),
        }
    }
}

/// What a frame tests next.
enum Next {
    /// A value against a node.
    Test(Word, u64),
    /// A field the record does not have.
    Missing,
    Done,
}

/// What looking at one value and one node gives.
enum Look {
    Answer(bool),
    /// The same question, of another node, in another scope.
    Instead(u64, Env),
    Inside(Inside),
}

/// Whether `ty`, a descriptor, describes a value. `scope` is the type
/// arguments of the function that asks, or null: what a type parameter
/// in `ty` stands for.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_is(value: Word, ty: u64, scope: Word) -> Word {
    unsafe { boolean(is(value, ty, Env::given(scope))) }
}

/// A value can be nested arbitrarily deep, and a type can follow it all
/// the way down, so the tests still to be made are kept in `frames`,
/// never on the stack.
unsafe fn is(mut value: Word, mut ty: u64, mut scope: Env) -> bool {
    let mut scopes: Vec<Scope> = Vec::new();
    let mut frames: Vec<Frame> = Vec::new();
    loop {
        let mut answer = match unsafe { look(value, ty, scope, &mut scopes) } {
            Look::Answer(answer) => answer,
            Look::Instead(node, outer) => {
                (ty, scope) = (node, outer);
                continue;
            }
            Look::Inside(inside) => {
                let frame = Frame {
                    inside,
                    next: 0,
                    scope,
                    entered: scopes.len(),
                };
                // Not what settles it: so far, nothing has.
                let unsettled = !frame.settled_by();
                frames.push(frame);
                unsettled
            }
        };
        // Hand the answer to whatever was waiting for it, until something
        // has another test to make.
        loop {
            let Some(frame) = frames.last_mut() else {
                return answer;
            };
            if answer != frame.settled_by() {
                match unsafe { frame.advance() } {
                    Next::Test(member, node) => {
                        scopes.truncate(frame.entered);
                        (value, ty, scope) = (member, node, frame.scope);
                        break;
                    }
                    Next::Missing => answer = false,
                    // Nothing settled it.
                    Next::Done => {}
                }
            }
            frames.pop();
        }
    }
}

/// Whether a function whose type depends on the type arguments it
/// holds is of a type `test` passes: that type is a node, read in those
/// arguments, and is compared, never named.
unsafe fn says(
    function: Word,
    scopes: &[Scope],
    test: impl FnOnce(&mut Types, At) -> bool,
) -> bool {
    unsafe { described(function).is_some_and(|ty| test(&mut Types::new(scopes), ty)) }
}

/// Tests `value` against a function type's node, generic or not. A
/// function's definition names its type, and so does the node: a
/// function is of that type and no other. Nothing is decided here of
/// one type being within another.
unsafe fn function(value: Word, ty: u64, scope: Env, scopes: &[Scope]) -> bool {
    use descriptor as d;
    unsafe {
        let node = |index: usize| (ty as usize as *const u64).add(index).read();
        // A method is the function reading it gives.
        if !matches!(kind(value), KIND_CLOSURE | KIND_METHOD) {
            return false;
        }
        // A generic function is of the type its definition names, as
        // any function is, until it holds its type arguments.
        if node(0) == d::GENERIC {
            let name = node(3 + node(1) as usize);
            return name != 0 && type_name(value) == name && generic(value);
        }
        let name = node(4 + node(1) as usize + node(3 + node(1) as usize) as usize);
        let named = name != 0 && type_name(value) == name;
        named || says(value, scopes, |types, said| types.same(said, (ty, scope)))
    }
}

/// Tests `value` against the node at `ty` alone: whatever the node says
/// of the values inside it is left to the caller.
unsafe fn look(value: Word, ty: u64, scope: Env, scopes: &mut Vec<Scope>) -> Look {
    use descriptor as d;
    unsafe {
        let ty = ty as usize as *const u64;
        let node = |index: usize| ty.add(index).read();
        let nodes =
            |start: usize, count: u64| core::slice::from_raw_parts(ty.add(start), count as usize);
        let inside = |holds: bool, inside: Inside| {
            if holds {
                Look::Inside(inside)
            } else {
                Look::Answer(false)
            }
        };
        let integer = value & 1 == 1 || kind(value) == KIND_INT;
        Look::Answer(match node(0) {
            d::UNKNOWN => true,
            d::NULL => value == NULL,
            d::BOOL => value == TRUE || value == FALSE,
            d::INT => integer,
            d::FLOAT => kind(value) == KIND_FLOAT,
            d::STRING => kind(value) == KIND_STRING,
            d::BYTES => kind(value) == KIND_BYTES,
            // Brands are tags, and equal tags are one object.
            d::HANDLE => kind(value) == KIND_HANDLE && words(value)[0] == node(1),
            d::TAG => kind(value) == KIND_TAG,
            // A method is the function reading it gives.
            d::CALLABLE => matches!(kind(value), KIND_CLOSURE | KIND_METHOD),
            d::LITERAL => match node(1) {
                d::LITERAL_BOOL => value == boolean(node(2) != 0),
                d::LITERAL_INT => integer && int(value) == node(2) as i64,
                d::LITERAL_FLOAT => kind(value) == KIND_FLOAT && payload(value).read() == node(2),
                d::LITERAL_TAG => value == node(2),
                d::LITERAL_BYTES => kind(value) == KIND_BYTES && bytes(value) == bytes(node(2)),
                _ => kind(value) == KIND_STRING && bytes(value) == bytes(node(2)),
            },
            d::RECORD => {
                let fields = nodes(2, node(1) * 2);
                let record = value;
                return inside(
                    kind(value) == KIND_RECORD,
                    Inside::Fields { record, fields },
                );
            }
            d::TUPLE => {
                let nodes = nodes(2, node(1));
                let holds = kind(value) == KIND_TUPLE && words(value).len() == nodes.len();
                let members = if holds { &*words(value) } else { &[] };
                return inside(holds, Inside::Members { nodes, members });
            }
            d::LIST => {
                let holds = kind(value) == KIND_LIST;
                let members = if holds { members(value) } else { &[] };
                let node = node(1);
                return inside(holds, Inside::Every { node, members });
            }
            d::UNION => {
                let nodes = nodes(2, node(1));
                return Look::Inside(Inside::Union { value, nodes });
            }
            d::APPLY => {
                scopes.push(Scope {
                    nodes: nodes(3, node(2)),
                    outer: scope,
                });
                return Look::Instead(node(1), Env::Entered(scopes.len() - 1));
            }
            d::PARAMETER => match argument(scopes, scope, node(1) as usize) {
                Some((node, outer)) => return Look::Instead(node, outer),
                None => false,
            },
            d::ALIAS => return Look::Instead(node(1), scope),
            // A cell holds exactly what it was made to hold, so the two
            // types must be the same, not one within the other.
            d::CELL => {
                kind(value) == KIND_CELL
                    && made_at(value).0 != 0
                    && Types::new(scopes).same(made_at(value), (node(1), scope))
            }
            // An iterator the runtime steps gives what it holds, which
            // is tested. One a function steps is whatever that function
            // says it is, as any function is.
            d::ITERATOR => {
                if kind(value) != KIND_ITERATOR {
                    return Look::Answer(false);
                }
                match tested(value) {
                    // Every iterator is one of whatever it gives.
                    Tested::Function(_)
                        if (node(1) as usize as *const u64).read() == d::UNKNOWN =>
                    {
                        true
                    }
                    Tested::Function(function) => {
                        let named = node(2) != 0 && type_name(function) == node(2);
                        let item = (node(1), scope);
                        named || says(function, scopes, |types, ty| types.steps(ty, item))
                    }
                    Tested::Items(members) => {
                        let node = node(1);
                        return inside(true, Inside::Every { node, members });
                    }
                }
            }
            d::GENERIC | d::FUNCTION => function(value, ty as u64, scope, scopes),
            _ => false,
        })
    }
}

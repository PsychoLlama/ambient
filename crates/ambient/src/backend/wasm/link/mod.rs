//! Linking the runtime into a module: the pieces the module reaches,
//! each given its place, and nothing else. See `spec/linking.md`.
//!
//! A program's pieces are at addresses and indices known when it is
//! put together. A unit's are not: it is loaded wherever its program
//! has room, so its data and its table entries are reached from where
//! a global says they start (`unit.rs`), and the functions its target
//! already has are imported by hash.

mod hash;
mod leb;
mod object;
mod unit;

use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;

pub use leb::leb128;
use leb::sleb128;
pub use object::Signature;
use object::{Body, Library, Target};

use crate::hash::Hash;

/// The bytes of memory below the runtime's data: the stack, which grows
/// down, so one that overflows leaves memory instead of writing over
/// what is next to it.
const STACK: u32 = 4 << 20;

/// The module a unit imports each function its target has from, by
/// hash, and the one it imports the address of each state from: zero
/// for a state its target lacks, which the unit then brings.
pub const PIECE_MODULE: &str = "piece";
pub const STATE_MODULE: &str = "state";

/// The runtime as a relocatable object: see `build.rs`.
const OBJECT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ambient_runtime.o"));

/// The runtime, read once.
fn library() -> Result<&'static Library, String> {
    static LIBRARY: OnceLock<Result<Library, String>> = OnceLock::new();
    LIBRARY
        .get_or_init(|| Library::read(OBJECT))
        .as_ref()
        .map_err(Clone::clone)
}

/// What a module has before the runtime's pieces.
pub struct Space {
    /// How many types.
    pub types: u32,
    /// How many globals it imports. A unit's pieces import more, and
    /// define theirs after those.
    pub globals: u32,
    /// The type of a function that takes and gives nothing.
    pub thunk: u32,
}

/// What a module is linked for.
pub enum For<'a> {
    /// A program: besides its roots it has each of these functions, if
    /// its roots reach the function it is paired with.
    Program(&'a [(&'static str, &'a str)]),
    /// A unit, for a target that has the functions of these hashes.
    Unit(&'a BTreeSet<Hash>),
}

/// A piece a module carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Carried {
    /// What the runtime's source calls it.
    pub name: String,
    pub hash: Hash,
    /// Its bytes, before it was placed.
    pub size: usize,
    /// Whether it is a function, which a target can be said to have.
    pub function: bool,
}

/// What a module tells of a piece it carries, for code loaded later.
pub enum Offer {
    /// A function, by its index.
    Function(u32),
    /// A state of a program, by its address.
    State(u32),
    /// A state of a unit, by the global that holds its address once
    /// the unit is loaded: the program's, if the program had one.
    Global(u32),
}

/// The pieces a module reaches, placed.
#[derive(Default)]
pub struct Linked {
    /// The types to add to the module's, in order.
    pub types: Vec<Signature>,
    /// The functions to import, the module's first: where from, and the
    /// type of each.
    pub imports: Vec<(String, String, u32)>,
    /// The states a unit imports the addresses of, by hash, as globals
    /// after those the module imports already.
    pub states: Vec<Hash>,
    /// How many globals a unit defines for its pieces, the first of
    /// the globals it defines. The first of them holds the address of
    /// its block.
    pub globals: u32,
    /// The functions to define, after the imports: the type and the
    /// body of each.
    pub functions: Vec<(u32, Vec<u8>)>,
    /// The function a unit's `init` calls first, to take the room its
    /// pieces need. Nothing gives that room back: what a unit brings
    /// of the runtime is the program's from then on.
    pub setup: Option<u32>,
    /// The functions the table holds: in a program from its second
    /// place on, and in a unit wherever its setup puts them.
    pub slots: Vec<u32>,
    /// What memory holds from [`Linked::start`] before anything runs,
    /// in a program; in a unit, what its block starts with.
    pub data: Vec<u8>,
    pub start: u32,
    /// The bytes of memory the runtime's data takes in all, counted
    /// from nothing.
    pub end: u32,
    /// Where the stack starts: its highest address.
    pub stack: u32,
    pub pieces: Vec<Carried>,
    /// What the module tells of its pieces, each by hash.
    pub offers: Vec<(Hash, Offer)>,
    /// In a unit, how far into its block its own copy of each state
    /// is, and each address in its data to fill in once it is loaded:
    /// how far into the block, and the number.
    own: Vec<u32>,
    fixups: Vec<(u32, Number)>,
    /// The index of each function that was asked for.
    named: HashMap<String, u32>,
}

impl Linked {
    /// The index of a function that was asked for by name.
    pub fn function(&self, name: &str) -> Result<u32, String> {
        let found = self.named.get(name).copied();
        found.ok_or_else(|| format!("the runtime has no `{name}`"))
    }

    /// Whether a function was linked under its name.
    pub fn has(&self, name: &str) -> bool {
        self.named.contains_key(name)
    }

    /// How many functions come before the module's own.
    pub fn count(&self) -> usize {
        self.imports.len() + self.functions.len()
    }
}

/// Where a reached piece is.
#[derive(Clone, Copy)]
enum Place {
    /// A function, by index.
    Function(u32),
    /// Data at an address.
    Address(u32),
    /// Data this far into a unit's block.
    Block(u32),
    /// A state, which a unit reaches through a global of its own: this
    /// one, counted from the first.
    State(u32),
}

/// A number a place holds: one known now, or one that is this far
/// from what a global holds once the module is loaded.
#[derive(Clone, Copy)]
enum Number {
    Plain(u32),
    Based(u32, i64),
}

/// Where each reached piece goes.
struct Places<'a> {
    library: &'a Library,
    of: HashMap<usize, Place>,
    slots: HashMap<usize, u32>,
    types: HashMap<u32, u32>,
    /// In a unit, the first global it defines for its pieces: the
    /// address of its block. The next is where its table entries
    /// start, and one for each state follows.
    block: Option<u32>,
}

impl Places<'_> {
    /// The number a place holds.
    fn number(&self, target: Target) -> Option<Number> {
        Some(match target {
            Target::Function(piece) => match self.of.get(&piece)? {
                Place::Function(index) => Number::Plain(*index),
                _ => return None,
            },
            Target::Slot(piece) => {
                let slot = self.slots.get(&piece).copied()?;
                match self.block {
                    // The table's first place is taken in a program,
                    // and a unit's entries start where it is told.
                    Some(block) => Number::Based(block + 1, i64::from(slot) - 1),
                    None => Number::Plain(slot),
                }
            }
            Target::Address(piece, offset) => match (self.of.get(&piece)?, self.block) {
                (Place::Address(base), _) => {
                    Number::Plain(u32::try_from(i64::from(*base) + offset).ok()?)
                }
                (Place::Block(at), Some(block)) => Number::Based(block, i64::from(*at) + offset),
                (Place::State(state), Some(block)) => Number::Based(block + 2 + state, offset),
                _ => return None,
            },
            Target::Type(ty) => Number::Plain(self.types.get(&ty).copied()?),
            // A module has one of each, its first.
            Target::Stack | Target::Table => Number::Plain(0),
        })
    }
}

/// Links the functions named `roots`, and everything they reach.
///
/// # Errors
///
/// Fails if the runtime has no function of one of those names.
pub fn link(roots: &BTreeSet<&str>, target: &For, space: &Space) -> Result<Linked, String> {
    let library = library()?;
    let piece = |name: &str| {
        let piece = library.named.get(name).copied();
        piece.ok_or_else(|| format!("the runtime has no `{name}`"))
    };
    let mut named = Vec::new();
    for root in roots {
        named.push((*root, piece(root)?));
    }
    // A function the target has is not looked into: what it reaches,
    // the target has too.
    let had = |piece: usize| match target {
        For::Unit(has) => {
            let function = matches!(library.pieces[piece].body, Body::Function { .. });
            function && has.contains(&library.hashes[piece])
        }
        For::Program(_) => false,
    };
    let mut reached = reach(library, named.iter().map(|(_, piece)| *piece), &had);
    if let For::Program(offered) = target {
        for (name, needs) in *offered {
            if reached.contains(&piece(needs)?) {
                named.push((*name, piece(name)?));
            }
        }
        reached = reach(library, named.iter().map(|(_, piece)| *piece), &had);
    }

    let mut linked = Linked {
        stack: STACK,
        ..Linked::default()
    };
    let mut places = Places {
        library,
        of: HashMap::new(),
        slots: HashMap::new(),
        types: HashMap::new(),
        block: None,
    };
    let defined = number(&reached, &had, space, &mut places, &mut linked)?;
    match target {
        For::Program(_) => lay_out(&reached, &mut linked, &mut places),
        For::Unit(_) => unit::lay_out(&reached, space, &mut linked, &mut places),
    }
    for (piece, ty) in &defined {
        let found = &library.pieces[*piece];
        let Body::Function { ty: of, code } = &found.body else {
            continue;
        };
        let params = library.types.get(*of as usize).map_or(0, |ty| ty.0.len());
        let body = unit::rewritten(found, code, params, &places)?;
        linked.functions.push((*ty, body));
    }
    for (name, piece) in named {
        if let Some(Place::Function(index)) = places.of.get(&piece) {
            linked.named.insert(name.to_owned(), *index);
        }
    }
    if matches!(target, For::Unit(_)) {
        unit::thunks(space, &mut linked)?;
    }
    describe(&reached, &had, &places, &mut linked);
    Ok(linked)
}

/// Numbers the functions and the types the reached pieces need, and
/// the table entries. Returns each function to define, with its type.
fn number(
    reached: &BTreeSet<usize>,
    had: &dyn Fn(usize) -> bool,
    space: &Space,
    places: &mut Places,
    linked: &mut Linked,
) -> Result<Vec<(usize, u32)>, String> {
    let library = places.library;
    let small = |value: usize| u32::try_from(value).map_err(|error| error.to_string());
    let mut types = HashMap::new();
    let mut ty = |ty: u32, linked: &mut Linked| {
        let next = space.types + u32::try_from(types.len()).unwrap_or(0);
        *types.entry(ty).or_insert_with(|| {
            linked.types.extend(library.types.get(ty as usize).cloned());
            next
        })
    };
    // Imports are numbered before the functions a module defines.
    let mut defined = Vec::new();
    for &piece in reached {
        let import = match &library.pieces[piece].body {
            Body::Import { module, field, ty } => (module.clone(), field.clone(), *ty),
            Body::Function { ty, .. } if had(piece) => {
                let hash = library.hashes[piece].to_string();
                (PIECE_MODULE.to_owned(), hash, *ty)
            }
            Body::Function { ty: of, .. } => {
                defined.push((piece, ty(*of, linked)));
                continue;
            }
            Body::Data { .. } => continue,
        };
        // Two pieces of one hash are one import.
        let same = |other: &(String, String, u32)| (&other.0, &other.1) == (&import.0, &import.1);
        let index = linked.imports.iter().position(same).unwrap_or_else(|| {
            let of = ty(import.2, linked);
            linked.imports.push((import.0, import.1, of));
            linked.imports.len() - 1
        });
        places.of.insert(piece, Place::Function(small(index)?));
    }
    for (position, (piece, _)) in defined.iter().enumerate() {
        let index = small(linked.imports.len() + position)?;
        places.of.insert(*piece, Place::Function(index));
    }
    for piece in reached.iter().filter(|piece| !had(**piece)) {
        for reloc in &library.pieces[*piece].relocs {
            match reloc.target {
                Target::Type(of) => {
                    ty(of, linked);
                }
                Target::Slot(function) if !places.slots.contains_key(&function) => {
                    let Some(Place::Function(index)) = places.of.get(&function) else {
                        continue;
                    };
                    linked.slots.push(*index);
                    places.slots.insert(function, small(linked.slots.len())?);
                }
                _ => {}
            }
        }
    }
    places.types = types;
    Ok(defined)
}

/// The pieces `roots` reach, themselves among them, in the order the
/// library has them. A piece `had` says the target has is reached and
/// not looked into.
fn reach(
    library: &Library,
    roots: impl Iterator<Item = usize>,
    had: &dyn Fn(usize) -> bool,
) -> BTreeSet<usize> {
    let mut reached = BTreeSet::new();
    let mut pending: Vec<usize> = roots.collect();
    while let Some(piece) = pending.pop() {
        if !reached.insert(piece) || had(piece) {
            continue;
        }
        for reloc in &library.pieces[piece].relocs {
            match reloc.target {
                Target::Function(next) | Target::Slot(next) | Target::Address(next, _) => {
                    pending.push(next);
                }
                Target::Type(_) | Target::Stack | Target::Table => {}
            }
        }
    }
    reached
}

/// The data pieces among `reached`: each with whether it starts as
/// zeros, its bytes, and its alignment. What starts as zeros goes
/// last, since memory starts as zeros.
fn data<'a>(library: &'a Library, reached: &BTreeSet<usize>) -> Vec<(bool, usize, &'a [u8], u32)> {
    let data = reached
        .iter()
        .filter_map(|piece| match &library.pieces[*piece].body {
            Body::Data { bytes, align, .. } => Some((
                bytes.iter().all(|byte| *byte == 0),
                *piece,
                bytes.as_slice(),
                *align,
            )),
            _ => None,
        });
    let mut data: Vec<_> = data.collect();
    data.sort_by_key(|(zeros, piece, ..)| (*zeros, *piece));
    data
}

/// Gives each data piece of a program its address and fills in what
/// memory starts with.
fn lay_out(reached: &BTreeSet<usize>, linked: &mut Linked, places: &mut Places) {
    let library = places.library;
    linked.start = STACK;
    let mut next = STACK;
    let mut filled = Vec::new();
    for (zeros, piece, bytes, align) in data(library, reached) {
        next = next.next_multiple_of(1 << align.min(16));
        places.of.insert(piece, Place::Address(next));
        if !zeros {
            filled.push((piece, next));
        }
        next += u32::try_from(bytes.len()).unwrap_or(0);
    }
    linked.end = next;
    for (piece, address) in filled {
        let found = &library.pieces[piece];
        let Body::Data { bytes, .. } = &found.body else {
            continue;
        };
        let at = (address - STACK) as usize;
        linked.data.resize(at, 0);
        linked.data.extend(bytes);
        for reloc in &found.relocs {
            let Some(Number::Plain(value)) = places.number(reloc.target) else {
                continue;
            };
            let start = at + reloc.offset;
            linked.data[start..start + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
}

/// Lists what was linked, and what of it code loaded later can use.
fn describe(
    reached: &BTreeSet<usize>,
    had: &dyn Fn(usize) -> bool,
    places: &Places,
    linked: &mut Linked,
) {
    let library = places.library;
    for &piece in reached {
        let found = &library.pieces[piece];
        let (size, state) = match &found.body {
            Body::Function { code, .. } => (code.len(), false),
            Body::Data { bytes, state, .. } => (bytes.len(), *state),
            Body::Import { .. } => continue,
        };
        if had(piece) {
            continue;
        }
        let hash = library.hashes[piece];
        let function = matches!(found.body, Body::Function { .. });
        linked.pieces.push(Carried {
            name: found.name.clone(),
            hash,
            size,
            function,
        });
        match places.of.get(&piece) {
            Some(Place::Function(index)) => linked.offers.push((hash, Offer::Function(*index))),
            Some(Place::Address(at)) if state => linked.offers.push((hash, Offer::State(*at))),
            Some(Place::State(index)) => {
                let global = places.block.unwrap_or(0) + 2 + index;
                linked.offers.push((hash, Offer::Global(global)));
            }
            _ => {}
        }
    }
}

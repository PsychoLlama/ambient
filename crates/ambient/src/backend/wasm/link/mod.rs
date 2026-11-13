//! Linking the runtime into a module: the pieces the module reaches,
//! each given its place, and nothing else. See `spec/linking.md`.

mod object;

use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;

pub use object::Signature;
use object::{Body, Form, Library, Piece, Target};

/// The bytes of memory below the runtime's data: the stack, which grows
/// down, so one that overflows leaves memory instead of writing over
/// what is next to it.
const STACK: u32 = 1 << 20;

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

/// What a module has before the runtime's pieces: how many types.
/// The runtime's functions are the module's first, after the ones it
/// imports for them.
pub struct Space {
    pub types: u32,
}

/// The pieces a module reaches, placed.
#[derive(Default)]
pub struct Linked {
    /// The types to add to the module's, in order.
    pub types: Vec<Signature>,
    /// The functions to import, the module's first: where from, and the
    /// type of each.
    pub imports: Vec<(String, String, u32)>,
    /// The functions to define, after the imports: the type and the
    /// body of each.
    pub functions: Vec<(u32, Vec<u8>)>,
    /// The functions the table holds from its second place on.
    pub slots: Vec<u32>,
    /// What memory holds from [`Linked::start`] before anything runs.
    pub data: Vec<u8>,
    pub start: u32,
    /// The bytes of memory the runtime's data takes in all, counted
    /// from nothing.
    pub end: u32,
    /// Where the stack starts: its highest address.
    pub stack: u32,
    /// Each piece that was linked: what the runtime's source calls it,
    /// and its bytes.
    pub pieces: Vec<(String, usize)>,
    /// The index of each function that was asked for.
    named: HashMap<String, u32>,
}

impl Linked {
    /// The index of a function that was asked for by name.
    pub fn function(&self, name: &str) -> Result<u32, String> {
        let found = self.named.get(name).copied();
        found.ok_or_else(|| format!("the runtime has no `{name}`"))
    }

    /// How many functions come before the module's own.
    pub fn count(&self) -> usize {
        self.imports.len() + self.functions.len()
    }
}

/// Where each reached piece goes.
#[derive(Default)]
struct Places {
    /// The index of each function piece, and the address of each data
    /// piece.
    of: HashMap<usize, u32>,
    slots: HashMap<usize, u32>,
    types: HashMap<u32, u32>,
}

/// Links the functions named `roots`, and everything they reach.
///
/// # Errors
///
/// Fails if the runtime has no function of one of those names.
pub fn link(roots: &BTreeSet<&str>, space: &Space) -> Result<Linked, String> {
    let library = library()?;
    let mut named = Vec::new();
    for root in roots {
        let piece = library.named.get(*root).copied();
        named.push((
            *root,
            piece.ok_or_else(|| format!("the runtime has no `{root}`"))?,
        ));
    }
    let reached = reach(library, named.iter().map(|(_, piece)| *piece));

    let mut linked = Linked {
        stack: STACK,
        ..Linked::default()
    };
    let mut places = Places::default();
    let mut ty = |ty: u32, linked: &mut Linked| {
        let next = space.types + u32::try_from(places.types.len()).unwrap_or(0);
        *places.types.entry(ty).or_insert_with(|| {
            linked.types.extend(library.types.get(ty as usize).cloned());
            next
        })
    };
    // Imports are numbered before the functions a module defines.
    let mut functions = Vec::new();
    for &piece in &reached {
        match &library.pieces[piece].body {
            Body::Import {
                module,
                field,
                ty: of,
            } => {
                let index =
                    u32::try_from(linked.imports.len()).map_err(|error| error.to_string())?;
                let import = (module.clone(), field.clone(), ty(*of, &mut linked));
                linked.imports.push(import);
                places.of.insert(piece, index);
            }
            Body::Function { ty: of, .. } => functions.push((piece, ty(*of, &mut linked))),
            Body::Data { .. } => {}
        }
    }
    for (position, (piece, _)) in functions.iter().enumerate() {
        let index =
            u32::try_from(linked.imports.len() + position).map_err(|error| error.to_string())?;
        places.of.insert(*piece, index);
    }
    for piece in reached.iter().map(|piece| &library.pieces[*piece]) {
        for reloc in &piece.relocs {
            match reloc.target {
                Target::Type(of) => {
                    ty(of, &mut linked);
                }
                Target::Slot(function) if !places.slots.contains_key(&function) => {
                    linked
                        .slots
                        .push(places.of.get(&function).copied().unwrap_or(0));
                    let slot = u32::try_from(linked.slots.len()).unwrap_or(0);
                    places.slots.insert(function, slot);
                }
                _ => {}
            }
        }
    }
    lay_out(library, &reached, &mut linked, &mut places);
    for piece in reached.iter().map(|piece| &library.pieces[*piece]) {
        let size = match &piece.body {
            Body::Function { code, .. } => code.len(),
            Body::Data { bytes, .. } => bytes.len(),
            Body::Import { .. } => 0,
        };
        linked.pieces.push((piece.name.clone(), size));
    }

    for (piece, ty) in functions {
        let found = &library.pieces[piece];
        let Body::Function { code, .. } = &found.body else {
            continue;
        };
        linked.functions.push((ty, patched(found, code, &places)?));
    }
    for (name, piece) in named {
        let index = places.of.get(&piece).copied().unwrap_or(0);
        linked.named.insert(name.to_owned(), index);
    }
    Ok(linked)
}

/// The pieces `roots` reach, themselves among them, in the order the
/// library has them.
fn reach(library: &Library, roots: impl Iterator<Item = usize>) -> BTreeSet<usize> {
    let mut reached = BTreeSet::new();
    let mut pending: Vec<usize> = roots.collect();
    while let Some(piece) = pending.pop() {
        if !reached.insert(piece) {
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

/// Gives each data piece its address and fills in what memory starts
/// with. What starts as zeros goes last, since memory starts as zeros.
fn lay_out(library: &Library, reached: &BTreeSet<usize>, linked: &mut Linked, places: &mut Places) {
    let data = reached
        .iter()
        .filter_map(|piece| match &library.pieces[*piece].body {
            Body::Data { bytes, align, .. } => {
                Some((bytes.iter().all(|byte| *byte == 0), *piece, bytes, *align))
            }
            _ => None,
        });
    let mut data: Vec<_> = data.collect();
    data.sort_by_key(|(zeros, piece, ..)| (*zeros, *piece));
    linked.start = STACK;
    let mut next = STACK;
    let mut filled = Vec::new();
    for (zeros, piece, bytes, align) in data {
        next = next.next_multiple_of(1 << align.min(16));
        places.of.insert(piece, next);
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
            let value = places.value(reloc.target).unwrap_or(0);
            let start = at + reloc.offset;
            linked.data[start..start + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
}

impl Places {
    /// The number a place holds.
    fn value(&self, target: Target) -> Option<u32> {
        match target {
            Target::Function(piece) => self.of.get(&piece).copied(),
            Target::Slot(piece) => self.slots.get(&piece).copied(),
            Target::Address(piece, offset) => {
                let base = i64::from(self.of.get(&piece).copied()?);
                u32::try_from(base + offset).ok()
            }
            Target::Type(ty) => self.types.get(&ty).copied(),
            // A module has one of each, its first.
            Target::Stack | Target::Table => Some(0),
        }
    }
}

/// A function's body with every place filled in, each as short as its
/// number allows.
fn patched(piece: &Piece, code: &[u8], places: &Places) -> Result<Vec<u8>, String> {
    let mut body = Vec::with_capacity(code.len());
    let mut copied = 0;
    for reloc in &piece.relocs {
        let value = places.value(reloc.target);
        let value = value.ok_or_else(|| format!("{} refers to what has no place", piece.name))?;
        body.extend(&code[copied..reloc.offset]);
        match reloc.form {
            Form::Unsigned => leb128(&mut body, u64::from(value)),
            Form::Signed => sleb128(&mut body, i64::from(value.cast_signed())),
            Form::Word => body.extend(value.to_le_bytes()),
        }
        copied = reloc.offset + reloc.form.len();
    }
    body.extend(&code[copied..]);
    Ok(body)
}

/// Writes an unsigned number seven bits to a byte.
pub fn leb128(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = value.to_le_bytes()[0] & 0x7f;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn sleb128(out: &mut Vec<u8>, mut value: i64) {
    loop {
        let byte = value.to_le_bytes()[0] & 0x7f;
        value >>= 7;
        let done = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
        out.push(if done { byte } else { byte | 0x80 });
        if done {
            return;
        }
    }
}

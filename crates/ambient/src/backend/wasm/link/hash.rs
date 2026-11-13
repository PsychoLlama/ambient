//! The name of each piece: a hash of its content and of the pieces it
//! refers to. See "Pieces" in `spec/linking.md`.
//!
//! Pieces refer to each other in cycles, so a piece cannot be hashed
//! after everything it refers to. Those that reach each other are
//! hashed together: each on its own first, with a reference into the
//! group standing for nothing, then the group in the order of those
//! named, and then each member as its place in the group.

use wasm_encoder::ValType;

use super::object::{Body, Form, Library, Piece, Signature, Target};
use crate::hash::{Hash, Hasher};

/// The hash of every piece of a library, by position.
pub fn hashes(library: &Library) -> Vec<Hash> {
    let mut named: Vec<Option<Hash>> = vec![None; library.pieces.len()];
    for group in groups(library) {
        let mut members: Vec<(Hash, usize)> = group
            .iter()
            .map(|piece| (alone(library, *piece, &named), *piece))
            .collect();
        members.sort();
        let mut whole = Hasher::new("pieces");
        for (index, (hash, piece)) in members.iter().enumerate() {
            whole.hash(hash);
            // Where in the group each reference into it lands.
            for target in library.pieces[*piece].relocs.iter().filter_map(piece_of) {
                let place = members.iter().position(|(_, member)| *member == target);
                whole.u64(place.map_or(u64::MAX, |place| place as u64));
            }
            whole.u64(index as u64);
        }
        let whole = whole.finish();
        for (index, (_, piece)) in members.iter().enumerate() {
            let mut hasher = Hasher::new("piece");
            hasher.hash(&whole).u64(index as u64);
            named[*piece] = Some(hasher.finish());
        }
    }
    let unnamed = Hasher::new("piece").finish();
    named
        .into_iter()
        .map(|hash| hash.unwrap_or(unnamed))
        .collect()
}

fn piece_of(reloc: &super::object::Reloc) -> Option<usize> {
    match reloc.target {
        Target::Function(piece) | Target::Slot(piece) | Target::Address(piece, _) => Some(piece),
        Target::Type(_) | Target::Stack | Target::Table => None,
    }
}

fn signature(hasher: &mut Hasher, signature: Option<&Signature>) {
    let code = |ty: &ValType| match ty {
        ValType::I32 => 1,
        ValType::I64 => 2,
        ValType::F32 => 3,
        ValType::F64 => 4,
        _ => 0,
    };
    let (params, results) = signature.cloned().unwrap_or_default();
    hasher.bytes(&params.iter().map(code).collect::<Vec<u8>>());
    hasher.bytes(&results.iter().map(code).collect::<Vec<u8>>());
}

/// A piece hashed on its own: its content, with the room for each
/// number it refers to left blank, and what each of those refers to if
/// that has a hash already.
fn alone(library: &Library, piece: usize, named: &[Option<Hash>]) -> Hash {
    let Piece { name, body, relocs } = &library.pieces[piece];
    let mut hasher = Hasher::new("piece alone");
    let blank = |bytes: &[u8]| {
        let mut bytes = bytes.to_vec();
        for reloc in relocs {
            bytes[reloc.offset..reloc.offset + reloc.form.len()].fill(0);
        }
        bytes
    };
    match body {
        Body::Function { ty, code } => {
            hasher.u64(0).bytes(&blank(code));
            signature(&mut hasher, library.types.get(*ty as usize));
        }
        Body::Import { module, field, ty } => {
            hasher
                .u64(1)
                .bytes(module.as_bytes())
                .bytes(field.as_bytes());
            signature(&mut hasher, library.types.get(*ty as usize));
        }
        Body::Data {
            bytes,
            align,
            state,
        } => {
            hasher.u64(2).bytes(&blank(bytes)).u64(u64::from(*align));
            // Two states that start alike are still two: a state is
            // the one of its name.
            hasher.bytes(if *state { name.as_bytes() } else { &[] });
        }
    }
    for reloc in relocs {
        hasher.u64(reloc.offset as u64).u64(match reloc.form {
            Form::Unsigned => 0,
            Form::Signed => 1,
            Form::Word => 2,
        });
        match reloc.target {
            Target::Function(target) => refer(&mut hasher, 0, named[target], 0),
            Target::Slot(target) => refer(&mut hasher, 1, named[target], 0),
            Target::Address(target, offset) => refer(&mut hasher, 2, named[target], offset),
            Target::Type(ty) => {
                hasher.u64(3);
                signature(&mut hasher, library.types.get(ty as usize));
            }
            Target::Stack => {
                hasher.u64(4);
            }
            Target::Table => {
                hasher.u64(5);
            }
        }
    }
    hasher.finish()
}

fn refer(hasher: &mut Hasher, kind: u64, hash: Option<Hash>, offset: i64) {
    hasher.u64(kind).u64(offset.cast_unsigned());
    if let Some(hash) = hash {
        hasher.hash(&hash);
    }
}

/// The pieces in groups that reach each other, each group after every
/// group it refers to.
fn groups(library: &Library) -> Vec<Vec<usize>> {
    /// Where a walk is: a piece, and how many of its references it has
    /// followed.
    struct Visit {
        piece: usize,
        next: usize,
    }
    let count = library.pieces.len();
    let targets = |piece: usize| library.pieces[piece].relocs.iter().filter_map(piece_of);
    // The order each piece was first met in, and the earliest piece it
    // is known to reach that is still open.
    let (mut met, mut low) = (vec![usize::MAX; count], vec![0; count]);
    let (mut open, mut is_open) = (Vec::new(), vec![false; count]);
    let (mut groups, mut clock) = (Vec::new(), 0);
    for root in 0..count {
        if met[root] != usize::MAX {
            continue;
        }
        let mut walk = vec![Visit {
            piece: root,
            next: 0,
        }];
        while let Some(visit) = walk.last_mut() {
            let piece = visit.piece;
            if visit.next == 0 {
                (met[piece], low[piece]) = (clock, clock);
                clock += 1;
                open.push(piece);
                is_open[piece] = true;
            }
            if let Some(target) = targets(piece).nth(visit.next) {
                visit.next += 1;
                if met[target] == usize::MAX {
                    walk.push(Visit {
                        piece: target,
                        next: 0,
                    });
                } else if is_open[target] {
                    low[piece] = low[piece].min(met[target]);
                }
                continue;
            }
            walk.pop();
            if let Some(parent) = walk.last() {
                low[parent.piece] = low[parent.piece].min(low[piece]);
            }
            if low[piece] == met[piece] {
                let mut group = Vec::new();
                while let Some(member) = open.pop() {
                    is_open[member] = false;
                    group.push(member);
                    if member == piece {
                        break;
                    }
                }
                groups.push(group);
            }
        }
    }
    groups
}

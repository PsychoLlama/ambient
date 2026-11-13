//! Placing pieces whose addresses are not known until they are loaded,
//! and filling in the places that refer to them.
//!
//! A unit's data goes in a block it takes from its program when it is
//! loaded, and its table entries wherever the table has room then. A
//! place that held an address holds how far into the block it is, and
//! the code adds where the block starts, which a global holds. A state
//! is reached through a global of its own, since it is the program's
//! if the program has one and the unit's only if not.

use std::collections::BTreeSet;

use wasm_encoder::{Function, HeapType, Instruction, MemArg, MemArgTarget};

use super::object::{Body, Form, Piece};
use super::{Linked, Number, Place, Places, Space, leb128, sleb128};

/// Gives each data piece of a unit its place in the unit's block, and
/// each state a global.
pub(super) fn lay_out(
    reached: &BTreeSet<usize>,
    space: &Space,
    linked: &mut Linked,
    places: &mut Places,
) {
    let library = places.library;
    let mut next = 0u32;
    let mut filled = Vec::new();
    for (zeros, piece, bytes, align) in super::data(library, reached) {
        next = next.next_multiple_of(1 << align.min(3));
        let state = matches!(library.pieces[piece].body, Body::Data { state: true, .. });
        if state {
            let global = u32::try_from(linked.states.len()).unwrap_or(0);
            places.of.insert(piece, Place::State(global));
            linked.states.push(library.hashes[piece]);
            linked.own.push(next);
        } else {
            places.of.insert(piece, Place::Block(next));
        }
        if !zeros {
            filled.push((piece, next));
        }
        next += u32::try_from(bytes.len()).unwrap_or(0);
    }
    linked.end = next;
    let states = u32::try_from(linked.states.len()).unwrap_or(0);
    places.block = Some(space.globals + states);
    linked.globals = 2 + states;
    for (piece, at) in filled {
        let found = &library.pieces[piece];
        let Body::Data { bytes, .. } = &found.body else {
            continue;
        };
        linked.data.resize(at as usize, 0);
        linked.data.extend(bytes);
    }
    // An address in the data is filled in once the unit is loaded.
    for &piece in reached {
        let at = match (places.of.get(&piece), &library.pieces[piece].body) {
            (Some(Place::Block(at)), _) => *at,
            (Some(Place::State(state)), _) => linked.own[*state as usize],
            _ => continue,
        };
        for reloc in &library.pieces[piece].relocs {
            if let Some(number) = places.number(reloc.target) {
                let offset = u32::try_from(reloc.offset).unwrap_or(0);
                linked.fixups.push((at + offset, number));
            }
        }
    }
}

fn memory(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 2,
        target: MemArgTarget::Memory(0),
        ordering: None,
    }
}

/// Pushes a number that is known only once the unit is loaded.
fn push(body: &mut Function, number: Number) {
    match number {
        Number::Plain(value) => {
            body.instruction(&Instruction::I32Const(value.cast_signed()));
        }
        Number::Based(global, offset) => {
            body.instruction(&Instruction::GlobalGet(global));
            // Addresses wrap, so the offset is taken as it is.
            #[allow(clippy::cast_possible_truncation)]
            body.instruction(&Instruction::I32Const(offset as i32));
            body.instruction(&Instruction::I32Add);
        }
    }
}

/// Adds the function a unit takes its room with: see
/// [`Linked::setup`].
pub(super) fn thunks(space: &Space, linked: &mut Linked) -> Result<(), String> {
    let small = |value: usize| u32::try_from(value).map_err(|error| error.to_string());
    let states = small(linked.states.len())?;
    let block = space.globals + states;
    let (slots, state) = (block + 1, block + 2);
    let words = i64::from(linked.end.div_ceil(8));
    let entries = small(linked.slots.len())?.cast_signed();

    let mut setup = Function::new([]);
    // The program's states first: allocating is one of them.
    for index in 0..states {
        setup.instruction(&Instruction::GlobalGet(space.globals + index));
        setup.instruction(&Instruction::GlobalSet(state + index));
    }
    if words > 0 {
        setup.instruction(&Instruction::I64Const(words));
        setup.instruction(&Instruction::Call(linked.function("amb_words")?));
        setup.instruction(&Instruction::GlobalSet(block));
    }
    if !linked.data.is_empty() {
        setup.instruction(&Instruction::GlobalGet(block));
        setup.instruction(&Instruction::I32Const(0));
        setup.instruction(&Instruction::I32Const(
            small(linked.data.len())?.cast_signed(),
        ));
        setup.instruction(&Instruction::MemoryInit {
            mem: 0,
            data_index: 1,
        });
    }
    // A state the program lacks is the unit's own.
    for (index, own) in (0..states).zip(&linked.own) {
        setup.instruction(&Instruction::GlobalGet(state + index));
        setup.instruction(&Instruction::I32Eqz);
        setup.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
        push(&mut setup, Number::Based(block, i64::from(*own)));
        setup.instruction(&Instruction::GlobalSet(state + index));
        setup.instruction(&Instruction::End);
    }
    if entries > 0 {
        setup.instruction(&Instruction::RefNull(HeapType::FUNC));
        setup.instruction(&Instruction::I32Const(entries));
        setup.instruction(&Instruction::TableGrow(0));
        setup.instruction(&Instruction::GlobalSet(slots));
        setup.instruction(&Instruction::GlobalGet(slots));
        setup.instruction(&Instruction::I32Const(0));
        setup.instruction(&Instruction::I32Const(entries));
        setup.instruction(&Instruction::TableInit {
            elem_index: 1,
            table: 0,
        });
    }
    for (at, number) in &linked.fixups {
        setup.instruction(&Instruction::GlobalGet(block));
        push(&mut setup, *number);
        setup.instruction(&Instruction::I32Store(memory(*at)));
    }
    setup.instruction(&Instruction::End);

    let first = small(linked.count())?;
    linked.setup = Some(first);
    linked.functions.push((space.thunk, setup.into_raw_body()));
    Ok(())
}

/// The first of the instructions that load from memory, the first
/// that store to it, and the first past those.
const LOAD: u8 = 0x28;
const STORE: u8 = 0x36;
const PAST: u8 = 0x3f;

/// The type of the value each store instruction takes, as a local of
/// that type is declared: `i32`, `i64`, `f32`, `f64`.
fn stored(opcode: u8) -> u8 {
    match opcode {
        0x37 | 0x3c..=0x3e => 0x7e,
        0x38 => 0x7d,
        0x39 => 0x7c,
        _ => 0x7f,
    }
}

/// Reads an unsigned number seven bits to a byte: the number, and how
/// many bytes it took.
fn unsigned(bytes: &[u8]) -> (u32, usize) {
    let (mut value, mut read) = (0u32, 0);
    for byte in bytes.iter().take(5) {
        value |= u32::from(byte & 0x7f) << (7 * read);
        read += 1;
        if byte & 0x80 == 0 {
            break;
        }
    }
    (value, read)
}

/// A function's body with every place filled in, each as short as its
/// number allows.
///
/// A place whose number is only known once the module is loaded is
/// followed by the instructions that add where it starts. One inside
/// an instruction that loads or stores cannot be: the address is under
/// it on the stack. There the address is added to before the
/// instruction, with the value a store takes kept in a local meanwhile.
pub(super) fn rewritten(
    piece: &Piece,
    code: &[u8],
    params: usize,
    places: &Places,
) -> Result<Vec<u8>, String> {
    let broken = |what: &str| format!("{} {what}", piece.name);
    let mut numbers = Vec::new();
    for reloc in &piece.relocs {
        let number = places.number(reloc.target);
        numbers.push(number.ok_or_else(|| broken("refers to what has no place"))?);
    }
    // The locals it declares: how many groups, and each a count and a
    // type.
    let (groups, mut at) = unsigned(code);
    let mut locals = params;
    for _ in 0..groups {
        let (count, read) = unsigned(&code[at..]);
        locals += count as usize;
        at += read + 1;
    }
    // One more local for each type a store keeps a value of.
    let mut kept: Vec<u8> = Vec::new();
    for (reloc, number) in piece.relocs.iter().zip(&numbers) {
        let based = matches!(number, Number::Based(..)) && reloc.form == Form::Unsigned;
        let opcode = code.get(reloc.offset.wrapping_sub(2)).copied().unwrap_or(0);
        if based && (STORE..PAST).contains(&opcode) && !kept.contains(&stored(opcode)) {
            kept.push(stored(opcode));
        }
    }
    let mut body = Vec::with_capacity(code.len() + 16);
    leb128(&mut body, u64::from(groups) + kept.len() as u64);
    body.extend(&code[unsigned(code).1..at]);
    for ty in &kept {
        body.extend([1, *ty]);
    }

    let mut copied = at;
    for (reloc, number) in piece.relocs.iter().zip(numbers) {
        let end = reloc.offset + reloc.form.len();
        match (number, reloc.form) {
            (Number::Plain(value), Form::Unsigned) => {
                body.extend(&code[copied..reloc.offset]);
                leb128(&mut body, u64::from(value));
            }
            (Number::Plain(value), Form::Signed) => {
                body.extend(&code[copied..reloc.offset]);
                sleb128(&mut body, i64::from(value.cast_signed()));
            }
            (Number::Based(global, offset), Form::Signed) => {
                body.extend(&code[copied..reloc.offset]);
                sleb128(&mut body, offset);
                body.push(0x23);
                leb128(&mut body, u64::from(global));
                body.push(0x6a);
            }
            (Number::Based(global, offset), Form::Unsigned) => {
                let start = reloc.offset.checked_sub(2).filter(|start| *start >= copied);
                let start = start.ok_or_else(|| broken("holds an address nothing can add to"))?;
                let (opcode, align) = (code[start], code[start + 1]);
                if !(LOAD..PAST).contains(&opcode) || align >= 0x80 {
                    return Err(broken("holds an address nothing can add to"));
                }
                let offset =
                    u32::try_from(offset).map_err(|_| broken("reaches before its data"))?;
                body.extend(&code[copied..start]);
                let keep = kept.iter().position(|ty| *ty == stored(opcode));
                let keep = keep.filter(|_| opcode >= STORE).map(|keep| locals + keep);
                if let Some(local) = keep {
                    body.push(0x21);
                    leb128(&mut body, local as u64);
                }
                body.push(0x23);
                leb128(&mut body, u64::from(global));
                body.push(0x6a);
                if let Some(local) = keep {
                    body.push(0x20);
                    leb128(&mut body, local as u64);
                }
                body.extend([opcode, align]);
                leb128(&mut body, u64::from(offset));
            }
            (_, Form::Word) => return Err(broken("holds a whole word in its code")),
        }
        copied = end;
    }
    body.extend(&code[copied..]);
    Ok(body)
}

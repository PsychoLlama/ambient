//! The functions of a module that lay its statics out in the runtime's
//! memory, and that give them back.

use ambient_runtime::{Entry, UNIT_LIST};
use wasm_encoder::{Function, HeapType, Instruction, ValType};

use super::emit::memory;
use super::{
    INTERN, STRING_NEW, STRING_PINNED, TAG, UNIT_RELEASE, UNIT_RETAIN, WORDS, WORDS_FREE, import,
};
use crate::backend::statics::{Field, Make, Place, Statics};

/// The globals a module's layout reads and writes, by index. A program
/// that nothing is loaded into has none of them.
#[derive(Default)]
pub(super) struct Globals {
    /// For each part, the first of the two globals that say where the
    /// program has it: its root, then its definition. Only in a unit.
    pub known: Vec<Option<u32>>,
    /// The program's list of units to reclaim. Only in a unit.
    pub unused: u32,
    /// How many externs the program has. Only in a unit.
    pub natives: u32,
    /// Where `init` notes the address of the block, and where in the
    /// table the module's functions start.
    pub base: Option<u32>,
    pub table: Option<u32>,
}

/// The locals of `init`: the address of the block the definitions are
/// laid out in, the address the text was copied to, where in the table
/// the functions start, and the entry.
const BASE: u32 = 0;
const TEXT: u32 = 1;
const TABLE: u32 = 2;
const ENTRY: u32 = 3;

fn small(value: usize) -> Result<i32, String> {
    i32::try_from(value).map_err(|error| error.to_string())
}

fn number(value: usize) -> Result<i64, String> {
    i64::try_from(value).map_err(|error| error.to_string())
}

/// Pushes the address of a word of the block, which a local holds the
/// address of.
fn address(body: &mut Function, base: u32, word: usize) -> Result<(), String> {
    body.instruction(&Instruction::LocalGet(base));
    body.instruction(&Instruction::I64ExtendI32U);
    body.instruction(&Instruction::I64Const(i64::from(small(word * 8)?)));
    body.instruction(&Instruction::I64Add);
    Ok(())
}

/// Pushes where a part is: where the program has it, if its host said
/// it does, and where the block has it if not.
fn part(
    body: &mut Function,
    globals: &Globals,
    plan: &Statics,
    part: usize,
    definition: bool,
) -> Result<(), String> {
    let (root, at) = plan.placed[part].unwrap_or_default();
    address(body, BASE, if definition { at } else { root })?;
    if let Some(Some(known)) = globals.known.get(part) {
        // The block's own, if the program's root is zero.
        body.instruction(&Instruction::GlobalGet(known + u32::from(definition)));
        body.instruction(&Instruction::GlobalGet(*known));
        body.instruction(&Instruction::I64Eqz);
        body.instruction(&Instruction::Select);
    }
    Ok(())
}

/// The function that lays out a module's parts in the runtime's memory
/// and returns the entry's closure, which the caller owns.
///
/// `positions` is where each of [`crate::engine::Parts::codes`] is
/// among the module's `functions` functions.
pub(super) fn initializer(
    plan: &Statics,
    positions: &[u64],
    functions: u32,
    globals: &Globals,
) -> Result<Function, String> {
    let locals = [ValType::I32, ValType::I32, ValType::I32, ValType::I64];
    let mut body = Function::new_with_locals_types(locals);
    // Room in the table for the module's functions, then the functions.
    body.instruction(&Instruction::RefNull(HeapType::FUNC));
    body.instruction(&Instruction::I32Const(functions.cast_signed()));
    body.instruction(&Instruction::TableGrow(0));
    body.instruction(&Instruction::LocalTee(TABLE));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(functions.cast_signed()));
    body.instruction(&Instruction::TableInit {
        elem_index: 0,
        table: 0,
    });
    body.instruction(&Instruction::I64Const(number(plan.words.max(1))?));
    body.instruction(&Instruction::Call(WORDS));
    body.instruction(&Instruction::LocalSet(BASE));
    // The text goes into memory the runtime allocated, since the runtime
    // owns the memory.
    let text = number(plan.text.len().div_ceil(8).max(1))?;
    body.instruction(&Instruction::I64Const(text));
    body.instruction(&Instruction::Call(WORDS));
    body.instruction(&Instruction::LocalTee(TEXT));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(small(plan.text.len())?));
    body.instruction(&Instruction::MemoryInit {
        mem: 0,
        data_index: 0,
    });
    for (global, local) in [(globals.base, BASE), (globals.table, TABLE)] {
        if let Some(global) = global {
            body.instruction(&Instruction::LocalGet(local));
            body.instruction(&Instruction::GlobalSet(global));
        }
    }

    for (word, field) in &plan.fields {
        body.instruction(&Instruction::LocalGet(BASE));
        match *field {
            Field::Word(value) => {
                body.instruction(&Instruction::I64Const(value.cast_signed()));
            }
            Field::Address(target) => address(&mut body, BASE, target)?,
            // Code is an index into the table.
            Field::Code(code) => {
                body.instruction(&Instruction::I64Const(positions[code].cast_signed()));
                body.instruction(&Instruction::LocalGet(TABLE));
                body.instruction(&Instruction::I64ExtendI32U);
                body.instruction(&Instruction::I64Add);
            }
            Field::Root(index) => part(&mut body, globals, plan, index, false)?,
            Field::Definition(index) => part(&mut body, globals, plan, index, true)?,
            Field::Native(native) => {
                body.instruction(&Instruction::I64Const(number(native)?));
                if plan.loaded {
                    body.instruction(&Instruction::GlobalGet(globals.natives));
                    body.instruction(&Instruction::I64Add);
                }
            }
            Field::Text {
                make,
                offset,
                length,
            } => {
                body.instruction(&Instruction::LocalGet(TEXT));
                body.instruction(&Instruction::I32Const(small(offset)?));
                body.instruction(&Instruction::I32Add);
                body.instruction(&Instruction::I64Const(number(length)?));
                body.instruction(&Instruction::Call(match make {
                    // A unit's strings go when it does.
                    Make::String if plan.loaded => STRING_NEW,
                    Make::String => STRING_PINNED,
                    Make::Field => INTERN,
                    Make::Tag => TAG,
                }));
            }
        }
        body.instruction(&Instruction::I64Store(memory(small(word * 8)?)));
    }

    let Some(Place::Root(entry)) = plan.entry else {
        return Err("the module has no entry".to_owned());
    };
    part(&mut body, globals, plan, entry, false)?;
    if plan.loaded {
        body.instruction(&Instruction::LocalSet(ENTRY));
        // The text was copied by whatever was made of it.
        body.instruction(&Instruction::LocalGet(TEXT));
        body.instruction(&Instruction::I64Const(text));
        body.instruction(&Instruction::Call(WORDS_FREE));
        // The unit's record says where it goes once nothing refers to
        // it. It is held while the entry is taken, so that a unit the
        // entry is not in goes there at once.
        body.instruction(&Instruction::LocalGet(BASE));
        body.instruction(&Instruction::GlobalGet(globals.unused));
        body.instruction(&Instruction::I64Store(memory(small(UNIT_LIST * 8)?)));
        address(&mut body, BASE, 0)?;
        body.instruction(&Instruction::Call(UNIT_RETAIN));
        body.instruction(&Instruction::LocalGet(ENTRY));
        body.instruction(&Instruction::Call(import(Entry::Retain)));
        address(&mut body, BASE, 0)?;
        body.instruction(&Instruction::Call(UNIT_RELEASE));
        body.instruction(&Instruction::LocalGet(ENTRY));
    }
    body.instruction(&Instruction::End);
    Ok(body)
}

/// The function that gives back what a unit's `init` took: the strings
/// it holds, its place in the table, and its block.
pub(super) fn reclaimer(
    plan: &Statics,
    functions: u32,
    globals: &Globals,
) -> Result<Function, String> {
    let (Some(base), Some(table)) = (globals.base, globals.table) else {
        return Err("a unit notes where it is".to_owned());
    };
    let mut body = Function::new_with_locals_types([]);
    for (word, field) in &plan.fields {
        if let Field::Text {
            make: Make::String, ..
        } = field
        {
            body.instruction(&Instruction::GlobalGet(base));
            body.instruction(&Instruction::I64Load(memory(small(word * 8)?)));
            body.instruction(&Instruction::Call(import(Entry::Release)));
        }
    }
    body.instruction(&Instruction::GlobalGet(table));
    body.instruction(&Instruction::RefNull(HeapType::FUNC));
    body.instruction(&Instruction::I32Const(functions.cast_signed()));
    body.instruction(&Instruction::TableFill(0));
    body.instruction(&Instruction::GlobalGet(base));
    body.instruction(&Instruction::I64Const(number(plan.words.max(1))?));
    body.instruction(&Instruction::Call(WORDS_FREE));
    body.instruction(&Instruction::End);
    Ok(body)
}

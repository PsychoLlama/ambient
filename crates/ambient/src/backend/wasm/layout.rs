//! The functions of a module that lay its statics out in the runtime's
//! memory, and that give them back.
//!
//! `init` writes each field of the plan into the block. A field can be
//! stored by code of its own, or be an entry of a stream one loop
//! reads (`stream.rs`). The loop costs what a dozen fields do and an
//! entry a third of what storing one does, so a module is given
//! whichever `init` is smaller: the one with the loop, for any program
//! that does anything.

use ambient_runtime::UNIT_LIST;
use wasm_encoder::{Function, HeapType, Instruction, ValType};

use super::code::{Calls, call};
use super::emit::memory;
use super::stream::{Entry, Frame, Kind, LOCALS, Stream};
use crate::backend::statics::{Field, Make, Place, Statics};

/// The globals a module's layout reads and writes, by index. A program
/// that nothing is loaded into has none of them.
#[derive(Clone, Default)]
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
    /// The function that makes room for the pieces of the runtime a
    /// unit brings, if it brings any.
    pub setup: Option<u32>,
}

/// The locals of `init`: the address of the block the definitions are
/// laid out in, the address the text was copied to, where in the table
/// the functions start, and the entry. Those of the loop that reads a
/// stream follow.
const BASE: u32 = 0;
const TEXT: u32 = 1;
const TABLE: u32 = 2;
const ENTRY: u32 = 3;
const LOOP: u32 = 4;

/// The most bytes of a stream `init` reads. It reads one off the
/// stack, which has four megabytes in all (`link::STACK`), so the
/// fields of a longer one are each stored by code.
const ROOM: usize = 1 << 20;

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

/// `init`, and what it reads.
pub(super) struct Laid {
    pub(super) body: Function,
    pub(super) calls: Calls,
    /// The fields it reads as a stream, which follow the text in the
    /// module's first data. Empty if it stores every field by code.
    pub(super) stream: Vec<u8>,
}

impl Laid {
    /// How many bytes of a module this is.
    fn size(&self) -> usize {
        self.body.byte_len() + self.stream.len()
    }
}

/// What `init` is written from.
struct Layout<'a> {
    plan: &'a Statics,
    /// Where each of [`crate::engine::Parts::codes`] is among the
    /// module's `functions` functions.
    positions: &'a [u64],
    functions: u32,
    globals: &'a Globals,
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
) -> Result<Laid, String> {
    let layout = Layout {
        plan,
        positions,
        functions,
        globals,
    };
    let stored = layout.emit(None)?;
    let stream = Stream::of(layout.entries());
    if stream.bytes.is_empty() || stream.room() > ROOM {
        return Ok(stored);
    }
    let streamed = layout.emit(Some(stream))?;
    Ok(if streamed.size() < stored.size() {
        streamed
    } else {
        stored
    })
}

impl Layout<'_> {
    /// The global that says where the program has a part, if its host
    /// may say it does: only in a unit.
    fn known(&self, part: usize) -> Option<u32> {
        self.globals.known.get(part).copied().flatten()
    }

    /// Where the block has a part: its definition, or its root.
    fn placed(&self, part: usize, definition: bool) -> usize {
        let (root, at) = self.plan.placed[part].unwrap_or_default();
        if definition { at } else { root }
    }

    /// A field as an entry of a stream, or nothing for one only code
    /// can store: where a part is that the program may have already,
    /// which a global of the part's own says.
    fn entry(&self, word: usize, field: Field) -> Option<Entry> {
        let entry = |kind, value| Entry {
            word,
            kind,
            value,
            length: 0,
        };
        let part = |part, definition| {
            let none = self.known(part).is_none();
            none.then(|| entry(Kind::Block, self.placed(part, definition) as u64 * 8))
        };
        match field {
            Field::Word(value) => Some(entry(Kind::Word, value)),
            Field::Address(target) => Some(entry(Kind::Block, target as u64 * 8)),
            Field::Code(code) => Some(entry(Kind::Table, self.positions[code])),
            Field::Root(index) => part(index, false),
            Field::Definition(index) => part(index, true),
            // A program's externs are the first there are.
            Field::Native(native) if self.plan.loaded => Some(entry(Kind::Native, native as u64)),
            Field::Native(native) => Some(entry(Kind::Word, native as u64)),
            Field::Text {
                make,
                offset,
                length,
            } => Some(Entry {
                length,
                ..entry(make.into(), offset as u64)
            }),
        }
    }

    /// Every field that can be an entry of a stream.
    fn entries(&self) -> Vec<Entry> {
        let fields = self.plan.fields.iter();
        fields
            .filter_map(|(word, field)| self.entry(*word, *field))
            .collect()
    }

    /// Pushes where a part is: where the program has it, if its host
    /// said it does, and where the block has it if not.
    fn part(&self, body: &mut Function, part: usize, definition: bool) -> Result<(), String> {
        address(body, BASE, self.placed(part, definition))?;
        if let Some(known) = self.known(part) {
            // The block's own, if the program's root is zero.
            body.instruction(&Instruction::GlobalGet(known + u32::from(definition)));
            body.instruction(&Instruction::GlobalGet(known));
            body.instruction(&Instruction::I64Eqz);
            body.instruction(&Instruction::Select);
        }
        Ok(())
    }

    /// Emits code that stores one field in its word of the block.
    fn store(
        &self,
        body: &mut Function,
        calls: &mut Calls,
        word: usize,
        field: Field,
    ) -> Result<(), String> {
        body.instruction(&Instruction::LocalGet(BASE));
        match field {
            Field::Word(value) => {
                body.instruction(&Instruction::I64Const(value.cast_signed()));
            }
            Field::Address(target) => address(body, BASE, target)?,
            // Code is an index into the table.
            Field::Code(code) => {
                let position = self.positions[code].cast_signed();
                body.instruction(&Instruction::I64Const(position));
                body.instruction(&Instruction::LocalGet(TABLE));
                body.instruction(&Instruction::I64ExtendI32U);
                body.instruction(&Instruction::I64Add);
            }
            Field::Root(index) => self.part(body, index, false)?,
            Field::Definition(index) => self.part(body, index, true)?,
            Field::Native(native) => {
                body.instruction(&Instruction::I64Const(number(native)?));
                if self.plan.loaded {
                    body.instruction(&Instruction::GlobalGet(self.globals.natives));
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
                call(body, calls, maker(make, self.plan.loaded));
            }
        }
        body.instruction(&Instruction::I64Store(memory(small(word * 8)?)));
        Ok(())
    }

    /// Emits what comes before any field is written: room in the
    /// table and in memory, and the text. Returns how many words the
    /// text was given.
    fn begin(&self, body: &mut Function, calls: &mut Calls) -> Result<i64, String> {
        let Self {
            plan,
            functions,
            globals,
            ..
        } = self;
        // The pieces of the runtime the module brings come first: the
        // rest of this calls them.
        if let Some(setup) = globals.setup {
            body.instruction(&Instruction::Call(setup));
        }
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
        call(body, calls, "amb_words");
        body.instruction(&Instruction::LocalSet(BASE));
        // The text goes into memory the runtime allocated, since the runtime
        // owns the memory.
        let text = number(plan.text.len().div_ceil(8).max(1))?;
        body.instruction(&Instruction::I64Const(text));
        call(body, calls, "amb_words");
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
        Ok(text)
    }

    /// Emits what follows the fields: the entry's closure is what
    /// `init` gives, and a unit is handed to what reclaims it.
    fn end(&self, body: &mut Function, calls: &mut Calls, text: i64) -> Result<(), String> {
        let Some(Place::Root(entry)) = self.plan.entry else {
            return Err("the module has no entry".to_owned());
        };
        self.part(body, entry, false)?;
        if self.plan.loaded {
            body.instruction(&Instruction::LocalSet(ENTRY));
            // The text was copied by whatever was made of it.
            body.instruction(&Instruction::LocalGet(TEXT));
            body.instruction(&Instruction::I64Const(text));
            call(body, calls, "amb_words_free");
            // The unit's record says where it goes once nothing refers to
            // it. It is held while the entry is taken, so that a unit the
            // entry is not in goes there at once.
            body.instruction(&Instruction::LocalGet(BASE));
            body.instruction(&Instruction::GlobalGet(self.globals.unused));
            body.instruction(&Instruction::I64Store(memory(small(UNIT_LIST * 8)?)));
            address(body, BASE, 0)?;
            call(body, calls, "amb_unit_retain");
            body.instruction(&Instruction::LocalGet(ENTRY));
            call(body, calls, "amb_retain");
            address(body, BASE, 0)?;
            call(body, calls, "amb_unit_release");
            body.instruction(&Instruction::LocalGet(ENTRY));
        }
        body.instruction(&Instruction::End);
        Ok(())
    }

    /// `init`, which reads `stream` for the fields that are entries of
    /// it and stores each of the rest by code; with no stream, all of
    /// them.
    fn emit(&self, stream: Option<Stream>) -> Result<Laid, String> {
        let mut locals = vec![ValType::I32, ValType::I32, ValType::I32, ValType::I64];
        if stream.is_some() {
            locals.extend(LOCALS);
        }
        let mut body = Function::new_with_locals_types(locals);
        let mut calls = Calls::new();
        let text = self.begin(&mut body, &mut calls)?;
        if let Some(stream) = &stream {
            let frame = Frame {
                base: BASE,
                text: TEXT,
                table: TABLE,
                first: LOOP,
                natives: self.globals.natives,
                loaded: self.plan.loaded,
            };
            stream.read(&mut body, &mut calls, &frame, self.plan.text.len())?;
        }
        for (word, field) in &self.plan.fields {
            if stream.is_none() || self.entry(*word, *field).is_none() {
                self.store(&mut body, &mut calls, *word, *field)?;
            }
        }
        self.end(&mut body, &mut calls, text)?;
        Ok(Laid {
            body,
            calls,
            stream: stream.map(|stream| stream.bytes).unwrap_or_default(),
        })
    }
}

/// The entry of the runtime that makes what a block's text is to be.
pub(super) fn maker(make: Make, loaded: bool) -> &'static str {
    match make {
        // A unit's strings and bytes go when it does.
        Make::String if loaded => "amb_string_new",
        Make::String => "amb_string_pinned",
        Make::Bytes if loaded => "amb_bytes_new",
        Make::Bytes => "amb_bytes_pinned",
        Make::Field => "amb_intern",
        Make::Tag => "amb_tag",
    }
}

/// The function that gives back what a unit's `init` took: the strings
/// it holds, its place in the table, and its block.
pub(super) fn reclaimer(
    plan: &Statics,
    functions: u32,
    globals: &Globals,
) -> Result<(Function, Calls), String> {
    let (Some(base), Some(table)) = (globals.base, globals.table) else {
        return Err("a unit notes where it is".to_owned());
    };
    let mut body = Function::new_with_locals_types([]);
    let mut calls = Calls::new();
    for (word, field) in &plan.fields {
        if let Field::Text {
            make: Make::String | Make::Bytes,
            ..
        } = field
        {
            body.instruction(&Instruction::GlobalGet(base));
            body.instruction(&Instruction::I64Load(memory(small(word * 8)?)));
            call(&mut body, &mut calls, "amb_release");
        }
    }
    body.instruction(&Instruction::GlobalGet(table));
    body.instruction(&Instruction::RefNull(HeapType::FUNC));
    body.instruction(&Instruction::I32Const(functions.cast_signed()));
    body.instruction(&Instruction::TableFill(0));
    body.instruction(&Instruction::GlobalGet(base));
    body.instruction(&Instruction::I64Const(number(plan.words.max(1))?));
    call(&mut body, &mut calls, "amb_words_free");
    body.instruction(&Instruction::End);
    Ok((body, calls))
}

//! A block's fields as data, and the loop that writes them.
//!
//! Storing a field with code of its own takes a dozen bytes or so, and
//! a program has a field for every word of every definition. Written
//! as a stream they take three or four bytes each, and one loop reads
//! them all: see [`Stream::read`].
//!
//! An entry is a head, a value, and for text a length, each a number
//! written seven bits to a byte. The head's low three bits are the
//! entry's [`Kind`], and the rest is how many words past the last
//! entry's word this one's is, so entries are in the order of their
//! words.

use std::collections::BTreeSet;

use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

use super::code::{Calls, call};
use super::emit::{STACK, memory};
use super::layout::maker;
use super::link::leb128;
use crate::backend::statics::Make;

/// What an entry's value is, and so what is stored for it. A kind's
/// number is what a head holds for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Kind {
    /// The word itself.
    Word = 0,
    /// How many bytes into the block an address is.
    Block = 1,
    /// Which of the module's functions a table index is.
    Table = 2,
    /// Which of the module's externs one is, among the program's.
    Native = 3,
    /// Where in the text something starts, which the runtime makes a
    /// word of: a string, bytes, a field name, or a tag. Its length
    /// follows.
    String = 4,
    Bytes = 5,
    Field = 6,
    Tag = 7,
}

/// How many bits of a head say its kind.
const KIND_BITS: u32 = 3;

impl From<Make> for Kind {
    fn from(make: Make) -> Self {
        match make {
            Make::String => Self::String,
            Make::Bytes => Self::Bytes,
            Make::Field => Self::Field,
            Make::Tag => Self::Tag,
        }
    }
}

impl Kind {
    /// What the runtime makes of the entry's text, if it is text.
    fn text(self) -> Option<Make> {
        match self {
            Self::String => Some(Make::String),
            Self::Bytes => Some(Make::Bytes),
            Self::Field => Some(Make::Field),
            Self::Tag => Some(Make::Tag),
            Self::Word | Self::Block | Self::Table | Self::Native => None,
        }
    }
}

/// One field: the word it is, and what goes there.
#[derive(Clone, Copy, Debug)]
pub(super) struct Entry {
    pub(super) word: usize,
    pub(super) kind: Kind,
    pub(super) value: u64,
    /// The length of the text, if the entry is text.
    pub(super) length: usize,
}

/// Fields as data.
pub(super) struct Stream {
    pub(super) bytes: Vec<u8>,
    /// The kinds the entries are of: the loop tests for no other.
    kinds: BTreeSet<Kind>,
}

/// The locals the loop keeps its place in, after those `init` has: the
/// address of the next byte of the stream and of the stream's end, the
/// address of the word being written, and what was last read.
pub(super) const LOCALS: [ValType; 7] = [
    ValType::I32,
    ValType::I32,
    ValType::I32,
    ValType::I64,
    ValType::I64,
    ValType::I64,
    ValType::I64,
];

/// Which locals and globals of `init` the loop uses.
pub(super) struct Frame {
    /// The address of the block, and of its text.
    pub(super) base: u32,
    pub(super) text: u32,
    /// Where in the table the module's functions start.
    pub(super) table: u32,
    /// The first of [`LOCALS`].
    pub(super) first: u32,
    /// The global that holds how many externs the program has.
    pub(super) natives: u32,
    /// Whether the block is a unit, whose text goes when it does.
    pub(super) loaded: bool,
}

impl Frame {
    fn cursor(&self) -> u32 {
        self.first
    }

    fn end(&self) -> u32 {
        self.first + 1
    }

    fn at(&self) -> u32 {
        self.first + 2
    }

    fn head(&self) -> u32 {
        self.first + 3
    }

    fn value(&self) -> u32 {
        self.first + 4
    }

    fn shift(&self) -> u32 {
        self.first + 5
    }

    fn byte(&self) -> u32 {
        self.first + 6
    }
}

fn small(value: usize) -> Result<i32, String> {
    i32::try_from(value).map_err(|error| error.to_string())
}

impl Stream {
    /// The stream of `entries`, in the order of their words. Two
    /// entries for one word stay in the order they were given, so the
    /// later is what the word holds.
    pub(super) fn of(mut entries: Vec<Entry>) -> Self {
        entries.sort_by_key(|entry| entry.word);
        let mut stream = Stream {
            bytes: Vec::new(),
            kinds: BTreeSet::new(),
        };
        let mut last = 0;
        for entry in entries {
            let ahead = (entry.word - last) as u64;
            leb128(&mut stream.bytes, ahead << KIND_BITS | entry.kind as u64);
            leb128(&mut stream.bytes, entry.value);
            if entry.kind.text().is_some() {
                leb128(&mut stream.bytes, entry.length as u64);
            }
            stream.kinds.insert(entry.kind);
            last = entry.word;
        }
        stream
    }

    /// How many bytes of the stack the stream takes while it is read.
    pub(super) fn room(&self) -> usize {
        self.bytes.len().next_multiple_of(16)
    }

    /// Emits the loop that writes every entry into the block. The
    /// stream is `offset` bytes into the module's first data.
    ///
    /// Data cannot be read where it is, and memory the runtime
    /// allocates is its to give back. So the stream is copied to the
    /// stack, which is the module's own, and is off it again before
    /// the loop is left.
    pub(super) fn read(
        &self,
        body: &mut Function,
        calls: &mut Calls,
        frame: &Frame,
        offset: usize,
    ) -> Result<(), String> {
        let (room, length) = (small(self.room())?, small(self.bytes.len())?);
        body.instruction(&Instruction::GlobalGet(STACK));
        body.instruction(&Instruction::I32Const(room));
        body.instruction(&Instruction::I32Sub);
        body.instruction(&Instruction::LocalTee(frame.cursor()));
        body.instruction(&Instruction::GlobalSet(STACK));
        body.instruction(&Instruction::LocalGet(frame.cursor()));
        body.instruction(&Instruction::I32Const(small(offset)?));
        body.instruction(&Instruction::I32Const(length));
        body.instruction(&Instruction::MemoryInit {
            mem: 0,
            data_index: 0,
        });
        body.instruction(&Instruction::LocalGet(frame.cursor()));
        body.instruction(&Instruction::I32Const(length));
        body.instruction(&Instruction::I32Add);
        body.instruction(&Instruction::LocalSet(frame.end()));
        body.instruction(&Instruction::LocalGet(frame.base));
        body.instruction(&Instruction::LocalSet(frame.at()));

        // A stream that is read has an entry, so the loop tests at its end.
        body.instruction(&Instruction::Loop(BlockType::Empty));
        number(body, frame, frame.head());
        // The head without its kind is eight times the words ahead:
        // how many bytes.
        body.instruction(&Instruction::LocalGet(frame.at()));
        body.instruction(&Instruction::LocalGet(frame.head()));
        body.instruction(&Instruction::I64Const(-(1 << KIND_BITS)));
        body.instruction(&Instruction::I64And);
        body.instruction(&Instruction::I32WrapI64);
        body.instruction(&Instruction::I32Add);
        body.instruction(&Instruction::LocalSet(frame.at()));
        number(body, frame, frame.value());
        body.instruction(&Instruction::LocalGet(frame.at()));
        self.stored(body, calls, frame);
        body.instruction(&Instruction::I64Store(memory(0)));
        body.instruction(&Instruction::LocalGet(frame.cursor()));
        body.instruction(&Instruction::LocalGet(frame.end()));
        body.instruction(&Instruction::I32LtU);
        body.instruction(&Instruction::BrIf(0));
        body.instruction(&Instruction::End);

        body.instruction(&Instruction::GlobalGet(STACK));
        body.instruction(&Instruction::I32Const(room));
        body.instruction(&Instruction::I32Add);
        body.instruction(&Instruction::GlobalSet(STACK));
        Ok(())
    }

    /// Pushes what the entry just read stores: a test for each kind
    /// the stream has but the last, which is what is left.
    fn stored(&self, body: &mut Function, calls: &mut Calls, frame: &Frame) {
        let last = self.kinds.len().saturating_sub(1);
        for (position, kind) in self.kinds.iter().enumerate() {
            if position < last {
                body.instruction(&Instruction::LocalGet(frame.head()));
                body.instruction(&Instruction::I64Const((1 << KIND_BITS) - 1));
                body.instruction(&Instruction::I64And);
                body.instruction(&Instruction::I64Const(*kind as i64));
                body.instruction(&Instruction::I64Eq);
                body.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
            }
            made(body, calls, frame, *kind);
            if position < last {
                body.instruction(&Instruction::Else);
            }
        }
        for _ in 0..last {
            body.instruction(&Instruction::End);
        }
    }
}

/// Pushes what an entry of `kind` stores, given its value.
fn made(body: &mut Function, calls: &mut Calls, frame: &Frame, kind: Kind) {
    let relative = |body: &mut Function, to: Instruction| {
        body.instruction(&to);
        body.instruction(&Instruction::LocalGet(frame.value()));
        body.instruction(&Instruction::I64Add);
    };
    match (kind, kind.text()) {
        (_, Some(make)) => {
            body.instruction(&Instruction::LocalGet(frame.text));
            body.instruction(&Instruction::LocalGet(frame.value()));
            body.instruction(&Instruction::I32WrapI64);
            body.instruction(&Instruction::I32Add);
            // The length is read over the value, which is on the stack.
            number(body, frame, frame.value());
            body.instruction(&Instruction::LocalGet(frame.value()));
            call(body, calls, maker(make, frame.loaded));
        }
        (Kind::Block, _) => {
            body.instruction(&Instruction::LocalGet(frame.base));
            relative(body, Instruction::I64ExtendI32U);
        }
        (Kind::Table, _) => {
            body.instruction(&Instruction::LocalGet(frame.table));
            relative(body, Instruction::I64ExtendI32U);
        }
        (Kind::Native, _) => relative(body, Instruction::GlobalGet(frame.natives)),
        _ => {
            body.instruction(&Instruction::LocalGet(frame.value()));
        }
    }
}

/// Emits a read of the number at the cursor into the local `into`,
/// which leaves the cursor after it.
fn number(body: &mut Function, frame: &Frame, into: u32) {
    let byte = MemArg {
        align: 0,
        ..memory(0)
    };
    for local in [into, frame.shift()] {
        body.instruction(&Instruction::I64Const(0));
        body.instruction(&Instruction::LocalSet(local));
    }
    body.instruction(&Instruction::Loop(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(frame.cursor()));
    body.instruction(&Instruction::I64Load8U(byte));
    body.instruction(&Instruction::LocalTee(frame.byte()));
    body.instruction(&Instruction::I64Const(0x7f));
    body.instruction(&Instruction::I64And);
    body.instruction(&Instruction::LocalGet(frame.shift()));
    body.instruction(&Instruction::I64Shl);
    body.instruction(&Instruction::LocalGet(into));
    body.instruction(&Instruction::I64Or);
    body.instruction(&Instruction::LocalSet(into));
    body.instruction(&Instruction::LocalGet(frame.shift()));
    body.instruction(&Instruction::I64Const(7));
    body.instruction(&Instruction::I64Add);
    body.instruction(&Instruction::LocalSet(frame.shift()));
    body.instruction(&Instruction::LocalGet(frame.cursor()));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(frame.cursor()));
    // A byte whose high bit is set has another after it.
    body.instruction(&Instruction::LocalGet(frame.byte()));
    body.instruction(&Instruction::I64Const(0x80));
    body.instruction(&Instruction::I64And);
    body.instruction(&Instruction::I64Const(0));
    body.instruction(&Instruction::I64Ne);
    body.instruction(&Instruction::BrIf(0));
    body.instruction(&Instruction::End);
}

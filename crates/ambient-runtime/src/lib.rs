//! The Ambient runtime. See `spec/backends.md`.
//!
//! Everything here is called by compiled code, through the C ABI, on both
//! backends. A function listed in [`Entry`] is an entry point: natively it is
//! reached through the table in a [`Context`], and in WebAssembly it is an
//! export of this crate built as a module.
//!
//! # Words
//!
//! Every value is one 64-bit [`Word`], on every backend:
//!
//! - low bit `1`: an integer that fits in 63 bits, shifted left by one;
//! - [`NULL`], [`FALSE`], [`TRUE`]: themselves;
//! - anything else non-zero: the address of an object, which is 8-aligned;
//! - [`UNWIND`], zero: not a value. A call that returns it is unwinding.
//!
//! # Ownership
//!
//! An entry point borrows its arguments and returns a word the caller owns.

// The runtime is where words become pointers and back. These casts are its
// job, and each is between a `u64` and a pointer or index that fits it.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_ptr_alignment,
    clippy::missing_safety_doc
)]

use std::alloc::{Layout, alloc, dealloc};
use std::sync::{Mutex, PoisonError};

pub type Word = u64;

/// Not a value: the result of a call that is unwinding.
pub const UNWIND: Word = 0;
pub const NULL: Word = 0b0010;
pub const FALSE: Word = 0b0110;
pub const TRUE: Word = 0b1010;

/// The least and greatest integers that are immediate.
pub const SMALL_MIN: i64 = -(1 << 62);
pub const SMALL_MAX: i64 = (1 << 62) - 1;

/// Why a call is unwinding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Fault {
    None = 0,
    /// Integer arithmetic left the range of `int`.
    Overflow = 1,
    /// No arm of a match was taken. Checking rules this out.
    NoMatch = 2,
}

impl Fault {
    #[must_use]
    pub fn from_code(code: u64) -> Self {
        match code {
            1 => Self::Overflow,
            2 => Self::NoMatch,
            _ => Self::None,
        }
    }
}

/// What kind of object a word refers to, or which immediate it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Kind {
    Null = 0,
    Bool = 1,
    Int = 2,
    Float = 3,
    String = 4,
    Tuple = 5,
    List = 6,
    Record = 7,
    Function = 8,
}

/// The state of one execution. Compiled code receives a pointer to it as
/// its first argument and passes it along.
///
/// The layout is part of the contract with compiled code: every field is
/// one word, at the index [`Context`]'s constants name.
#[repr(C)]
pub struct Context {
    /// Natively, the address of the entry point table. Unused in
    /// WebAssembly, where entry points are imports.
    pub entries: u64,
    /// The [`Fault`] that started the current unwinding.
    pub fault: u64,
}

impl Context {
    /// Byte offsets of the fields.
    pub const ENTRIES: i32 = 0;
    pub const FAULT: i32 = 8;
}

/// The entry points, in table order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Entry {
    Retain,
    Release,
    IntAdd,
    IntSubtract,
    IntMultiply,
    IntLess,
    FloatAdd,
    FloatSubtract,
    FloatMultiply,
    FloatLess,
    Equal,
    IntNew,
    FloatNew,
    Fault,
    SequenceNew,
    SequenceSet,
    SequenceGet,
    SequenceIs,
    RecordNew,
    RecordSet,
    RecordGet,
    RecordHas,
    RecordIs,
    ClosureNew,
    ClosureSet,
    Is,
}

impl Entry {
    pub const ALL: [Self; 26] = [
        Self::Retain,
        Self::Release,
        Self::IntAdd,
        Self::IntSubtract,
        Self::IntMultiply,
        Self::IntLess,
        Self::FloatAdd,
        Self::FloatSubtract,
        Self::FloatMultiply,
        Self::FloatLess,
        Self::Equal,
        Self::IntNew,
        Self::FloatNew,
        Self::Fault,
        Self::SequenceNew,
        Self::SequenceSet,
        Self::SequenceGet,
        Self::SequenceIs,
        Self::RecordNew,
        Self::RecordSet,
        Self::RecordGet,
        Self::RecordHas,
        Self::RecordIs,
        Self::ClosureNew,
        Self::ClosureSet,
        Self::Is,
    ];

    /// The name the entry point is exported under.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Retain => "amb_retain",
            Self::Release => "amb_release",
            Self::IntAdd => "amb_int_add",
            Self::IntSubtract => "amb_int_subtract",
            Self::IntMultiply => "amb_int_multiply",
            Self::IntLess => "amb_int_less",
            Self::FloatAdd => "amb_float_add",
            Self::FloatSubtract => "amb_float_subtract",
            Self::FloatMultiply => "amb_float_multiply",
            Self::FloatLess => "amb_float_less",
            Self::Equal => "amb_equal",
            Self::IntNew => "amb_int_new",
            Self::FloatNew => "amb_float_new",
            Self::Fault => "amb_fault",
            Self::SequenceNew => "amb_sequence_new",
            Self::SequenceSet => "amb_sequence_set",
            Self::SequenceGet => "amb_sequence_get",
            Self::SequenceIs => "amb_sequence_is",
            Self::RecordNew => "amb_record_new",
            Self::RecordSet => "amb_record_set",
            Self::RecordGet => "amb_record_get",
            Self::RecordHas => "amb_record_has",
            Self::RecordIs => "amb_record_is",
            Self::ClosureNew => "amb_closure_new",
            Self::ClosureSet => "amb_closure_set",
            Self::Is => "amb_is",
        }
    }

    /// Whether the entry point takes the context as its first argument.
    #[must_use]
    pub fn takes_context(self) -> bool {
        matches!(
            self,
            Self::IntAdd | Self::IntSubtract | Self::IntMultiply | Self::Fault
        )
    }

    /// How many words it takes, besides the context.
    #[must_use]
    pub fn arity(self) -> usize {
        match self {
            Self::Retain
            | Self::Release
            | Self::IntNew
            | Self::FloatNew
            | Self::Fault
            | Self::RecordNew
            | Self::RecordIs => 1,
            Self::SequenceSet | Self::SequenceIs | Self::RecordSet | Self::ClosureSet => 3,
            _ => 2,
        }
    }

    /// Whether it returns a word. The others return nothing.
    #[must_use]
    pub fn returns(self) -> bool {
        !matches!(
            self,
            Self::Retain | Self::Release | Self::SequenceSet | Self::RecordSet | Self::ClosureSet
        )
    }

    /// The native address of the entry point.
    #[must_use]
    pub fn address(self) -> usize {
        match self {
            Self::Retain => amb_retain as *const () as usize,
            Self::Release => amb_release as *const () as usize,
            Self::IntAdd => amb_int_add as *const () as usize,
            Self::IntSubtract => amb_int_subtract as *const () as usize,
            Self::IntMultiply => amb_int_multiply as *const () as usize,
            Self::IntLess => amb_int_less as *const () as usize,
            Self::FloatAdd => amb_float_add as *const () as usize,
            Self::FloatSubtract => amb_float_subtract as *const () as usize,
            Self::FloatMultiply => amb_float_multiply as *const () as usize,
            Self::FloatLess => amb_float_less as *const () as usize,
            Self::Equal => amb_equal as *const () as usize,
            Self::IntNew => amb_int_new as *const () as usize,
            Self::FloatNew => amb_float_new as *const () as usize,
            Self::Fault => amb_fault as *const () as usize,
            Self::SequenceNew => amb_sequence_new as *const () as usize,
            Self::SequenceSet => amb_sequence_set as *const () as usize,
            Self::SequenceGet => amb_sequence_get as *const () as usize,
            Self::SequenceIs => amb_sequence_is as *const () as usize,
            Self::RecordNew => amb_record_new as *const () as usize,
            Self::RecordSet => amb_record_set as *const () as usize,
            Self::RecordGet => amb_record_get as *const () as usize,
            Self::RecordHas => amb_record_has as *const () as usize,
            Self::RecordIs => amb_record_is as *const () as usize,
            Self::ClosureNew => amb_closure_new as *const () as usize,
            Self::ClosureSet => amb_closure_set as *const () as usize,
            Self::Is => amb_is as *const () as usize,
        }
    }
}

// Objects

const KIND_INT: u32 = 1;
const KIND_FLOAT: u32 = 2;
const KIND_CLOSURE: u32 = 3;
const KIND_STRING: u32 = 4;
const KIND_TUPLE: u32 = 5;
const KIND_LIST: u32 = 6;
const KIND_RECORD: u32 = 7;

/// The count of an object that is never reclaimed.
const PINNED: u32 = u32::MAX;

/// The layout of a closure and of the definition it refers to, for
/// compiled code. Every field is one word; offsets are in bytes.
///
/// A closure is an object whose payload is its definition and then its
/// captures. A definition is not an object: it is the code, the arity, and
/// then one closure per slot.
pub mod layout {
    /// From a closure to its definition.
    pub const CLOSURE_DEFINITION: i32 = 16;
    /// From a closure to its first capture.
    pub const CLOSURE_CAPTURES: i32 = 24;
    /// The words in a closure that captures nothing.
    pub const CLOSURE_WORDS: usize = 3;
    /// From a definition to its code.
    pub const DEFINITION_CODE: i32 = 0;
    /// From a definition to its first slot.
    pub const DEFINITION_SLOTS: i32 = 16;
    /// The words in a definition with no slots.
    pub const DEFINITION_WORDS: usize = 2;
}

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
}

/// Writes the header of a closure that is never reclaimed into `words`,
/// which must hold at least [`layout::CLOSURE_WORDS`].
pub fn pinned_closure(words: &mut [u64], definition: u64) {
    words[0] = u64::from(PINNED) | (u64::from(KIND_CLOSURE) << 32);
    words[1] = 8;
    words[2] = definition;
}

/// What every object starts with. The payload follows, in words.
#[repr(C)]
struct Header {
    count: u32,
    kind: u32,
    /// The payload's size in bytes.
    size: u64,
}

const HEADER: usize = size_of::<Header>();

fn layout(size: usize) -> Layout {
    // The size is one this module chose and the alignment is a constant, so
    // this cannot fail; an empty layout would only make allocation fail.
    Layout::from_size_align(HEADER + size, 8).unwrap_or(Layout::new::<Header>())
}

fn is_object(word: Word) -> bool {
    word != UNWIND && word.trailing_zeros() >= 3
}

fn header(word: Word) -> *mut Header {
    word as usize as *mut Header
}

unsafe fn payload(word: Word) -> *mut u64 {
    unsafe { (word as usize as *mut u8).add(HEADER).cast() }
}

/// A new object of `kind` with one reference and `size` payload bytes.
unsafe fn object(kind: u32, size: usize) -> Word {
    unsafe {
        let pointer = alloc(layout(size)).cast::<Header>();
        if pointer.is_null() {
            std::process::abort();
        }
        pointer.write(Header {
            count: 1,
            kind,
            size: size as u64,
        });
        pointer as usize as Word
    }
}

fn small(value: i64) -> Option<Word> {
    (SMALL_MIN..=SMALL_MAX)
        .contains(&value)
        .then_some(((value << 1) | 1) as Word)
}

fn boolean(value: bool) -> Word {
    if value { TRUE } else { FALSE }
}

/// The integer a word holds. The word must be an integer.
unsafe fn int(word: Word) -> i64 {
    if word & 1 == 1 {
        (word as i64) >> 1
    } else {
        unsafe { payload(word).cast::<i64>().read() }
    }
}

unsafe fn float(word: Word) -> f64 {
    unsafe { f64::from_bits(payload(word).read()) }
}

fn fault(context: *mut Context, fault: Fault) -> Word {
    unsafe {
        (*context).fault = fault as u64;
    }
    UNWIND
}

// Entry points

/// Takes one more reference to a value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_retain(word: Word) {
    if is_object(word) {
        unsafe {
            let header = header(word);
            if (*header).count != PINNED {
                (*header).count += 1;
            }
        }
    }
}

/// Gives up one reference to a value, reclaiming it if that was the last.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_release(word: Word) {
    // A value can be nested arbitrarily deep, so the values an object held
    // are queued here, never released by recursion.
    let mut pending = vec![word];
    while let Some(word) = pending.pop() {
        if !is_object(word) {
            continue;
        }
        unsafe {
            let header = header(word);
            if (*header).count == PINNED {
                continue;
            }
            (*header).count -= 1;
            if (*header).count != 0 {
                continue;
            }
            let held = match (*header).kind {
                KIND_TUPLE | KIND_LIST | KIND_RECORD => words(word),
                // The first word is the definition, which is not a value.
                KIND_CLOSURE => &words(word)[1..],
                _ => &[],
            };
            pending.extend_from_slice(held);
            let size = (*header).size as usize;
            dealloc(header.cast(), layout(size));
        }
    }
}

/// The payload of an object, as words.
unsafe fn words<'a>(word: Word) -> &'a mut [u64] {
    unsafe {
        let length = (*header(word)).size as usize / 8;
        std::slice::from_raw_parts_mut(payload(word), length)
    }
}

unsafe fn kind(word: Word) -> u32 {
    if is_object(word) {
        unsafe { (*header(word)).kind }
    } else {
        0
    }
}

unsafe fn bytes<'a>(word: Word) -> &'a [u8] {
    unsafe {
        let length = (*header(word)).size as usize;
        std::slice::from_raw_parts(payload(word).cast::<u8>(), length)
    }
}

/// An integer of any size as a value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_new(value: i64) -> Word {
    small(value).unwrap_or_else(|| unsafe {
        let word = object(KIND_INT, 8);
        payload(word).cast::<i64>().write(value);
        word
    })
}

/// A float, given as its bits, as a value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_new(bits: u64) -> Word {
    unsafe {
        let word = object(KIND_FLOAT, 8);
        payload(word).write(bits);
        word
    }
}

unsafe fn arithmetic(
    context: *mut Context,
    left: Word,
    right: Word,
    operation: fn(i64, i64) -> Option<i64>,
) -> Word {
    unsafe {
        match operation(int(left), int(right)) {
            Some(result) => amb_int_new(result),
            None => fault(context, Fault::Overflow),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_add(context: *mut Context, left: Word, right: Word) -> Word {
    unsafe { arithmetic(context, left, right, i64::checked_add) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_subtract(context: *mut Context, left: Word, right: Word) -> Word {
    unsafe { arithmetic(context, left, right, i64::checked_sub) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_multiply(context: *mut Context, left: Word, right: Word) -> Word {
    unsafe { arithmetic(context, left, right, i64::checked_mul) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_less(left: Word, right: Word) -> Word {
    unsafe { boolean(int(left) < int(right)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_add(left: Word, right: Word) -> Word {
    unsafe { amb_float_new((float(left) + float(right)).to_bits()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_subtract(left: Word, right: Word) -> Word {
    unsafe { amb_float_new((float(left) - float(right)).to_bits()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_multiply(left: Word, right: Word) -> Word {
    unsafe { amb_float_new((float(left) * float(right)).to_bits()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_less(left: Word, right: Word) -> Word {
    unsafe { boolean(float(left) < float(right)) }
}

/// Structural equality.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_equal(left: Word, right: Word) -> Word {
    unsafe { boolean(equal(left, right)) }
}

unsafe fn equal(left: Word, right: Word) -> bool {
    if left == right {
        return true;
    }
    if !is_object(left) || !is_object(right) {
        return false;
    }
    // TODO: this recurses as deep as the values nest.
    unsafe {
        let all = |left: &[u64], right: &[u64]| {
            left.len() == right.len() && left.iter().zip(right).all(|(l, r)| equal(*l, *r))
        };
        match ((*header(left)).kind, (*header(right)).kind) {
            (KIND_INT, KIND_INT) => int(left) == int(right),
            // Equality is structural: a float equals itself, bit for bit.
            (KIND_FLOAT, KIND_FLOAT) => payload(left).read() == payload(right).read(),
            (KIND_STRING, KIND_STRING) => bytes(left) == bytes(right),
            // A record's keys are interned and kept in order, so equal
            // records have equal keys at equal positions.
            (KIND_TUPLE, KIND_TUPLE) | (KIND_LIST, KIND_LIST) | (KIND_RECORD, KIND_RECORD) => {
                all(words(left), words(right))
            }
            (KIND_CLOSURE, KIND_CLOSURE) => {
                let (left, right) = (words(left), words(right));
                left[0] == right[0] && all(&left[1..], &right[1..])
            }
            _ => false,
        }
    }
}

// Strings

/// A string that is never reclaimed, copied from `length` bytes at
/// `pointer`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_pinned(pointer: *const u8, length: u64) -> Word {
    unsafe {
        let word = object(KIND_STRING, length as usize);
        (*header(word)).count = PINNED;
        std::ptr::copy_nonoverlapping(pointer, payload(word).cast::<u8>(), length as usize);
        word
    }
}

/// A string copied from `length` bytes at `pointer`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_new(pointer: *const u8, length: u64) -> Word {
    unsafe {
        let word = object(KIND_STRING, length as usize);
        std::ptr::copy_nonoverlapping(pointer, payload(word).cast::<u8>(), length as usize);
        word
    }
}

/// The one string with these bytes that stands for a field name. Two field
/// names are the same name exactly when their words are equal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_intern(pointer: *const u8, length: u64) -> Word {
    static NAMES: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    unsafe {
        let name = std::slice::from_raw_parts(pointer, length as usize);
        let mut names = NAMES.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(known) = names.iter().find(|known| bytes(**known as Word) == name) {
            return *known as Word;
        }
        let word = amb_string_pinned(pointer, length);
        names.push(word as usize);
        word
    }
}

// Tuples and lists

/// A tuple, or a list if `list` is not zero, of `length` nulls. Fill it
/// with [`amb_sequence_set`] before anything else sees it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_sequence_new(list: u64, length: u64) -> Word {
    unsafe {
        let kind = if list == 0 { KIND_TUPLE } else { KIND_LIST };
        let word = object(kind, length as usize * 8);
        words(word).fill(NULL);
        word
    }
}

/// Puts `value` at `index` of a sequence under construction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_sequence_set(sequence: Word, index: u64, value: Word) {
    unsafe {
        amb_retain(value);
        words(sequence)[index as usize] = value;
    }
}

/// The member at `index`, which must exist.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_sequence_get(sequence: Word, index: u64) -> Word {
    unsafe {
        let value = words(sequence)[index as usize];
        amb_retain(value);
        value
    }
}

/// Whether a value is a tuple of exactly `length` members, or, if `list`
/// is not zero, a list of any length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_sequence_is(value: Word, list: u64, length: u64) -> Word {
    unsafe {
        boolean(match kind(value) {
            KIND_TUPLE => list == 0 && words(value).len() as u64 == length,
            KIND_LIST => list != 0,
            _ => false,
        })
    }
}

// Records
//
// A record is its fields as (key, value) pairs, ordered by key. A key is
// an interned string, so the order is by address: arbitrary, but the same
// for every record in one execution.

/// A record with room for `length` fields. Fill it with
/// [`amb_record_set`] before anything else sees it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_record_new(length: u64) -> Word {
    unsafe {
        let word = object(KIND_RECORD, length as usize * 16);
        words(word).fill(0);
        word
    }
}

/// Adds a field to a record under construction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_record_set(record: Word, key: Word, value: Word) {
    unsafe {
        amb_retain(value);
        let fields = words(record);
        // Unfilled pairs have a zero key and sit at the end.
        let filled = fields.chunks(2).take_while(|pair| pair[0] != 0).count();
        let position = fields[..filled * 2]
            .chunks(2)
            .position(|pair| pair[0] > key)
            .unwrap_or(filled);
        fields.copy_within(position * 2..filled * 2, position * 2 + 2);
        fields[position * 2] = key;
        fields[position * 2 + 1] = value;
    }
}

unsafe fn find(record: Word, key: Word) -> Option<Word> {
    unsafe {
        words(record)
            .chunks(2)
            .find(|pair| pair[0] == key)
            .map(|pair| pair[1])
    }
}

/// The field `key` of a record, which must have it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_record_get(record: Word, key: Word) -> Word {
    unsafe {
        let value = find(record, key).unwrap_or(NULL);
        amb_retain(value);
        value
    }
}

/// Whether a value is a record with the field `key`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_record_has(value: Word, key: Word) -> Word {
    unsafe { boolean(kind(value) == KIND_RECORD && find(value, key).is_some()) }
}

/// Whether a value is a record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_record_is(value: Word) -> Word {
    unsafe { boolean(kind(value) == KIND_RECORD) }
}

// Type tests

/// The arguments a generic type definition is being read at.
struct Arguments<'a> {
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
unsafe fn is(value: Word, ty: *const u64, arguments: Option<&Arguments>) -> bool {
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
            d::LITERAL => match node(1) {
                d::LITERAL_BOOL => value == boolean(node(2) != 0),
                d::LITERAL_INT => integer && int(value) == node(2) as i64,
                d::LITERAL_FLOAT => kind(value) == KIND_FLOAT && payload(value).read() == node(2),
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

// Closures

/// A closure over `definition` with room for `captures` values. Fill it
/// with [`amb_closure_set`] before anything else sees it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_closure_new(definition: u64, captures: u64) -> Word {
    unsafe {
        let word = object(KIND_CLOSURE, (captures as usize + 1) * 8);
        let fields = words(word);
        fields.fill(NULL);
        fields[0] = definition;
        word
    }
}

/// Puts `value` in capture `index` of a closure under construction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_closure_set(closure: Word, index: u64, value: Word) {
    unsafe {
        amb_retain(value);
        words(closure)[index as usize + 1] = value;
    }
}

/// Starts unwinding with a fault. Always returns [`UNWIND`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_fault(context: *mut Context, code: u64) -> Word {
    fault(context, Fault::from_code(code))
}

// For the host: making a context and reading results.

/// A new context. Free it with [`amb_context_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_context_new() -> *mut Context {
    Box::into_raw(Box::new(Context {
        entries: 0,
        fault: 0,
    }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_context_free(context: *mut Context) {
    unsafe {
        drop(Box::from_raw(context));
    }
}

/// The [`Fault`] code of the last unwinding in this context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_context_fault(context: *mut Context) -> u64 {
    unsafe { (*context).fault }
}

/// The [`Kind`] of a value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_kind(word: Word) -> u64 {
    let kind = if word & 1 == 1 {
        Kind::Int
    } else if word == NULL {
        Kind::Null
    } else if word == TRUE || word == FALSE {
        Kind::Bool
    } else {
        match unsafe { (*header(word)).kind } {
            KIND_FLOAT => Kind::Float,
            KIND_STRING => Kind::String,
            KIND_TUPLE => Kind::Tuple,
            KIND_LIST => Kind::List,
            KIND_RECORD => Kind::Record,
            KIND_CLOSURE => Kind::Function,
            _ => Kind::Int,
        }
    };
    kind as u64
}

/// Room for `count` words that is never reclaimed, zeroed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_words(count: u64) -> *mut u64 {
    Box::leak(vec![0u64; count as usize].into_boxed_slice()).as_mut_ptr()
}

/// The integer a value holds. The value must be an integer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_int_value(word: Word) -> i64 {
    unsafe { int(word) }
}

/// The bits of the float a value holds. The value must be a float.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_float_bits(word: Word) -> u64 {
    unsafe { payload(word).read() }
}

/// How many members a tuple or list has, fields a record has, or bytes a
/// string has.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_length(word: Word) -> u64 {
    unsafe {
        let size = (*header(word)).size;
        match (*header(word)).kind {
            KIND_STRING => size,
            KIND_RECORD => size / 16,
            _ => size / 8,
        }
    }
}

/// The address of a string's bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_string_bytes(word: Word) -> *const u8 {
    unsafe { payload(word).cast() }
}

/// The word at `index` of an object's payload, borrowed: a member of a
/// tuple or list, or, for a record, a key at an even index and its value
/// at the next.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_word_at(word: Word, index: u64) -> Word {
    unsafe { words(word)[index as usize] }
}

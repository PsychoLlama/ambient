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
}

impl Entry {
    pub const ALL: [Self; 14] = [
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
            Self::Retain | Self::Release | Self::IntNew | Self::FloatNew | Self::Fault => 1,
            _ => 2,
        }
    }

    /// Whether it returns a word. The others return nothing.
    #[must_use]
    pub fn returns(self) -> bool {
        !matches!(self, Self::Retain | Self::Release)
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
        }
    }
}

// Objects

const KIND_INT: u32 = 1;
const KIND_FLOAT: u32 = 2;
const KIND_CLOSURE: u32 = 3;

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
    if !is_object(word) {
        return;
    }
    unsafe {
        let header = header(word);
        if (*header).count == PINNED {
            return;
        }
        (*header).count -= 1;
        if (*header).count == 0 {
            let size = (*header).size as usize;
            dealloc(header.cast(), layout(size));
        }
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
    unsafe {
        match ((*header(left)).kind, (*header(right)).kind) {
            (KIND_INT, KIND_INT) => int(left) == int(right),
            // Equality is structural: a float equals itself, bit for bit.
            (KIND_FLOAT, KIND_FLOAT) => payload(left).read() == payload(right).read(),
            _ => false,
        }
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

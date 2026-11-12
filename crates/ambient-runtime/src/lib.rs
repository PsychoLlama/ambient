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

mod abort;
mod closure;
mod entry;
mod host;
mod is;
mod native;
mod number;
mod object;
mod record;
mod sequence;
mod text;

pub use abort::*;
pub use closure::*;
pub use entry::Entry;
pub use host::*;
pub use is::{amb_is, descriptor};
pub use native::*;
pub use number::*;
pub use object::{amb_release, amb_retain, layout, pinned_closure};
pub use record::*;
pub use sequence::*;
pub use text::*;

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
    /// An abort handler was performed after its provide had exited.
    Expired = 3,
    /// Not a fault: an abort is on its way to the provide that handles it.
    Abort = 4,
    /// Native code reported that it failed.
    Native = 5,
}

impl Fault {
    #[must_use]
    pub fn from_code(code: u64) -> Self {
        match code {
            1 => Self::Overflow,
            2 => Self::NoMatch,
            3 => Self::Expired,
            4 => Self::Abort,
            5 => Self::Native,
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
    /// While an abort unwinds: the token of the handler it is going to, and
    /// the arguments it was performed with, as a tuple this context owns.
    pub abort_token: u64,
    pub abort_args: u64,
    /// Natively, the address of the function that runs native code for
    /// this execution: it takes what [`amb_native`] takes. Unused in
    /// WebAssembly, where that function is an import.
    pub native: u64,
    /// Whatever the host wants to find again when native code is called.
    pub host: u64,
}

impl Context {
    /// Byte offsets of the fields.
    pub const ENTRIES: i32 = 0;
    pub const FAULT: i32 = 8;
}

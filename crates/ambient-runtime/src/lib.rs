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
#![no_std]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_ptr_alignment,
    clippy::missing_safety_doc
)]

extern crate alloc;
#[cfg(not(target_arch = "wasm32"))]
extern crate std;

mod abort;
mod boundary;
mod bytes;
mod cell;
mod cleanup;
mod closure;
mod convert;
mod entry;
mod handle;
#[cfg(target_arch = "wasm32")]
mod heap;
mod host;
mod is;
mod iterator;
mod list;
mod native;
mod number;
mod object;
mod record;
mod same;
mod scope;
mod sequence;
mod state;
mod task;
mod text;
mod trace;
mod unit;

pub use abort::*;
pub use boundary::*;
pub use bytes::*;
pub use cell::*;
pub use cleanup::*;
pub use closure::*;
pub use convert::*;
pub use entry::Entry;
pub use handle::{amb_handle_new, amb_handle_next, amb_handle_number};
pub use host::*;
pub use is::{amb_is, descriptor};
pub use iterator::{
    Source, amb_int_range, amb_iterator_advance, amb_iterator_hold, amb_iterator_new,
    amb_iterator_of, amb_iterator_stepper, amb_list_iterate, amb_string_iterate,
};
pub use list::*;
pub use native::*;
pub use number::*;
pub use object::{amb_release, amb_retain, fixed_closure, layout};
pub use record::*;
pub use scope::{amb_scope_new, amb_scope_set, fixed_scope};
pub use sequence::*;
pub use task::{DEPTH, FRAME, NATIVE, amb_call_among, amb_call_back, amb_depth, amb_enter};
pub use task::{
    FORSAKEN, LIMITS, RESUMING, RUNNING, SUSPENDING, WAITING, amb_all, amb_answer, amb_await,
    amb_bounce, amb_budget_back, amb_budget_caught, amb_budget_current, amb_budget_enter,
    amb_budget_leave, amb_budget_switch, amb_capture, amb_charge, amb_charge_read, amb_charge_size,
    amb_charge_tasks, amb_first, amb_frame_drop, amb_frame_load, amb_frame_point, amb_frame_push,
    amb_frame_save, amb_group_close, amb_group_end, amb_group_new, amb_native_wait, amb_refuse,
    amb_schedule, amb_spawn, amb_start, amb_strand, amb_tail, amb_tail_arg,
};
pub use text::*;
pub use trace::{TRACE_LIMIT, amb_trace};
pub use unit::*;

pub type Word = u64;

/// Not a value: the result of a call that is unwinding.
pub const UNWIND: Word = 0;

/// The most a function is called with, its bindings among them: the
/// runtime calls a function of each arity up to this from outside
/// compiled code (`task/tail.rs`).
pub const ARGUMENTS: usize = 16;
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
    /// Not a fault: a task is unwinding because it was cancelled. A host
    /// sees this only when every task was waiting for something that
    /// would never happen.
    Cancelled = 6,
    /// A cleanup aborted to a provide outside it.
    CleanupAbort = 7,
    /// Not a fault: a budget ran out, and what ran inside it is unwinding
    /// to it.
    Exhausted = 8,
    /// An integer was divided by zero.
    DivideByZero = 9,
    /// Not a fault: a function is leaving to make the call it ends
    /// with.
    Tail = 10,
    /// Calls nested deeper than [`DEPTH`] allows.
    Deep = 11,
    /// A task could go on only once the world answered, while native
    /// code it had called was on the stack.
    Pinned = 12,
}

impl Fault {
    /// Every way of unwinding but [`Self::None`].
    pub const ALL: &[Self] = &[
        Self::Overflow,
        Self::NoMatch,
        Self::Expired,
        Self::Abort,
        Self::Native,
        Self::Cancelled,
        Self::CleanupAbort,
        Self::Exhausted,
        Self::DivideByZero,
        Self::Tail,
        Self::Deep,
        Self::Pinned,
    ];

    #[must_use]
    pub fn from_code(code: u64) -> Self {
        match code {
            1 => Self::Overflow,
            2 => Self::NoMatch,
            3 => Self::Expired,
            4 => Self::Abort,
            5 => Self::Native,
            6 => Self::Cancelled,
            7 => Self::CleanupAbort,
            8 => Self::Exhausted,
            9 => Self::DivideByZero,
            10 => Self::Tail,
            11 => Self::Deep,
            12 => Self::Pinned,
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
    Cell = 9,
    Tag = 10,
    /// Not a value a program can name: what an abort handler is bound
    /// to.
    Token = 11,
    /// Not a value a program can name: the type arguments a generic
    /// function was made at, which follow what it captured.
    Types = 12,
    /// A method a record holds: a function with room for the record,
    /// which reading the field fills in.
    Method = 13,
    /// An iterator: its payload is its [`Source`], an integer, and
    /// the two values that keeps.
    Iterator = 14,
    /// Bytes that need not be text.
    Bytes = 15,
    /// A handle: the brand it was minted under, a tag, and the number
    /// its host knows it by, which is no value.
    Handle = 16,
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
    /// Whether tasks are running, suspending, or resuming: see
    /// [`RUNNING`].
    pub mode: u64,
    /// The tasks of this execution. Opaque to compiled code.
    pub scheduler: u64,
    /// While a fault unwinds: the frames it has left so far, as a value
    /// this context owns, or zero.
    pub trace: u64,
    /// Where the host keeps the first of the program's live cells, or
    /// zero if it keeps none: see [`amb_cells_drop`].
    pub cells: u64,
    /// While a function leaves for a call: the code to call. The callee
    /// is where an abort's token goes.
    pub tail: u64,
    /// The arguments of that call, which this context owns, and how
    /// many they are: see [`amb_tail_arg`].
    pub tail_args: [u64; ARGUMENTS],
    pub tail_count: u64,
    /// How deep the function that is making a call is: see
    /// [`amb_enter`].
    pub depth: u64,
}

impl Context {
    /// Byte offsets of the fields.
    pub const ENTRIES: i32 = 0;
    pub const FAULT: i32 = 8;
    pub const MODE: i32 = 48;
    pub const DEPTH: i32 = core::mem::offset_of!(Self, depth) as i32;
}

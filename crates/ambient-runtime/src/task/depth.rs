//! How deep calls nest: what makes recursion without end a fault and
//! not a crash. See "Calls" in `spec/backends.md`.
//!
//! Depth is counted in words, not calls: a function weighs what it
//! keeps, so the count bounds the stack a program takes on any backend.
//! A function that makes calls learns how deep it is as it starts
//! ([`amb_enter`]) and says so before each call ([`amb_depth`]), which
//! is where what it calls counts from. Nothing is undone on the way
//! out, so no way of leaving a function can lose count.

use crate::object::{fault, small};
use crate::{Context, Fault, NULL, Word};

/// The deepest calls nest, in words. A host gives a program stack for
/// this before it runs one: several times this many words natively,
/// where a frame holds more than the function's locals.
pub const DEPTH: u64 = 400_000;

/// What a function weighs besides its locals: room for what a backend
/// and the runtime keep for a call.
pub const FRAME: u64 = 32;

/// What native code that calls back into its program weighs: room for
/// what it and its host keep between the call it was made by and the
/// call it makes.
pub const NATIVE: u64 = 4096;

/// The depth of what is called from outside compiled code.
pub(super) const NOWHERE: Word = 1;

/// Called as a function that makes calls starts, with what it weighs.
/// Returns how deep it then is, for [`amb_depth`], or unwinds with
/// [`Fault::Deep`] if that is deeper than [`DEPTH`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_enter(context: *mut Context, weight: u64) -> Word {
    unsafe {
        let depth = (*context).depth.saturating_add(weight);
        match small(depth.cast_signed()) {
            Some(word) if depth <= DEPTH => word,
            _ => fault(context, Fault::Deep),
        }
    }
}

/// Says how deep the function about to make a call is: what
/// [`amb_enter`] gave it, which is always an immediate. Returns null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_depth(context: *mut Context, depth: Word) -> Word {
    unsafe {
        (*context).depth = depth >> 1;
        NULL
    }
}

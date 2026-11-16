//! Abort: unwinding to the provide that bound a handler. See
//! `spec/backends.md`.
//!
//! A provide makes a token for each abort handler it binds, and puts the
//! token where a handler would go. Performing the ability records the token
//! and the arguments in the context and unwinds; each provide on the way
//! asks whether the abort is its own.

use crate::object::{KIND_TOKEN, amb_release, amb_retain, fault, object, payload};
use crate::{Context, Fault, NULL, UNWIND, Word};

/// What a token holds: whether its provide is still there, and whether
/// an abort to it gathers a trace for its handler.
const LIVE: u64 = 1;
const TRACED: u64 = 2;

/// A token that has not expired. With `traced` not zero, an abort to it
/// gathers a trace as it unwinds: its handler takes one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_token_new(traced: u64) -> Word {
    unsafe {
        let word = object(KIND_TOKEN, 8);
        payload(word).write(if traced == 0 { LIVE } else { LIVE | TRACED });
        word
    }
}

/// Expires a token: its provide is over. Returns null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_token_expire(token: Word) -> Word {
    unsafe {
        payload(token).write(payload(token).read() & !LIVE);
    }
    NULL
}

/// Whether the unwinding under way is an abort whose handler takes its
/// trace.
pub(crate) unsafe fn traced(context: *mut Context) -> bool {
    unsafe {
        (*context).fault == Fault::Abort as u64
            && payload((*context).abort_token).read() & TRACED != 0
    }
}

/// The trace of the abort that was just caught, as the list its handler
/// takes. The caller owns it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_traced(context: *mut Context) -> Word {
    unsafe { crate::trace::take(context) }
}

/// Starts unwinding to the provide that holds `token`, with `args`, a
/// tuple. Always returns [`UNWIND`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_abort(context: *mut Context, token: Word, args: Word) -> Word {
    unsafe {
        if payload(token).read() & LIVE == 0 {
            return fault(context, Fault::Expired);
        }
        amb_retain(args);
        (*context).abort_token = token;
        (*context).abort_args = args;
        fault(context, Fault::Abort)
    }
}

/// Called by a provide that its body unwound through. Expires `token`. If
/// the unwinding is an abort to `token`, ends it and returns the arguments,
/// which the caller owns; if not, returns [`UNWIND`] and it goes on.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_caught(context: *mut Context, token: Word) -> Word {
    unsafe {
        amb_token_expire(token);
        let aborting = (*context).fault == Fault::Abort as u64;
        if !aborting || (*context).abort_token != token {
            return UNWIND;
        }
        let args = (*context).abort_args;
        (*context).fault = Fault::None as u64;
        (*context).abort_token = 0;
        (*context).abort_args = 0;
        args
    }
}

/// Gives up whatever an unwinding that never arrived left in the context:
/// the arguments of an abort, and the trace of a fault.
pub(crate) unsafe fn abandon(context: *mut Context) {
    unsafe {
        let args = (*context).abort_args;
        if args != 0 {
            amb_release(args);
        }
        amb_release(core::mem::take(&mut (*context).trace));
    }
}

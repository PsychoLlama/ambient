//! Abort: unwinding to the provide that bound a handler. See
//! `spec/backends.md`.
//!
//! A provide makes a token for each abort handler it binds, and puts the
//! token where a handler would go. Performing the ability records the token
//! and the arguments in the context and unwinds; each provide on the way
//! asks whether the abort is its own.

use crate::object::{KIND_TOKEN, amb_release, amb_retain, fault, object, payload};
use crate::{Context, Fault, NULL, UNWIND, Word};

/// A token that has not expired.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_token_new() -> Word {
    unsafe {
        let word = object(KIND_TOKEN, 8);
        payload(word).write(1);
        word
    }
}

/// Expires a token: its provide is over. Returns null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_token_expire(token: Word) -> Word {
    unsafe {
        payload(token).write(0);
    }
    NULL
}

/// Starts unwinding to the provide that holds `token`, with `args`, a
/// tuple. Always returns [`UNWIND`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_abort(context: *mut Context, token: Word, args: Word) -> Word {
    unsafe {
        if payload(token).read() == 0 {
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
        payload(token).write(0);
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

/// Gives up whatever an abort that never arrived left in the context.
pub(crate) unsafe fn abandon(context: *mut Context) {
    unsafe {
        let args = (*context).abort_args;
        if args != 0 {
            amb_release(args);
        }
    }
}

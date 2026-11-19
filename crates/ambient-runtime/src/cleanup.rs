//! Cleanups: code that runs when a block exits, by any route. See
//! `docs/spec/cleanup.md`.
//!
//! A cleanup that runs because its block is unwinding has to leave the
//! unwinding as it found it. [`amb_hold`] takes the unwinding out of the
//! context, as a value the block keeps while the cleanup runs, and
//! [`amb_rethrow`] puts it back.
//!
//! A task inside a cleanup is not cancelled until it leaves: every entry
//! point here that starts a cleanup masks cancellation, and the one that
//! ends it unmasks.

use crate::abort::abandon;
use crate::number::amb_int_new;
use crate::object::{amb_release, amb_retain, fault, int, words};
use crate::sequence::{amb_sequence_new, amb_sequence_set};
use crate::task::mask;
use crate::trace::{attach, detach};
use crate::{Context, Fault, NULL, UNWIND, Word};

/// For a block that unwound and has a cleanup to run: ends the unwinding
/// for now, and returns it as a value for [`amb_rethrow`]. Enters the
/// cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_hold(context: *mut Context) -> Word {
    unsafe {
        let held = amb_sequence_new(0, 4);
        let code = amb_int_new((*context).fault.cast_signed());
        amb_sequence_set(held, 0, code);
        amb_release(code);
        // The token is kept alive by its provide, and now by this too.
        let token = core::mem::take(&mut (*context).abort_token);
        amb_sequence_set(held, 1, if token == 0 { NULL } else { token });
        let args = core::mem::take(&mut (*context).abort_args);
        if args != 0 {
            amb_sequence_set(held, 2, args);
            amb_release(args);
        }
        let trace = detach(context);
        amb_sequence_set(held, 3, trace);
        amb_release(trace);
        (*context).fault = Fault::None as u64;
        mask(context, true);
        held
    }
}

/// Leaves the cleanup and goes on with the unwinding `held` is, whatever
/// the cleanup did: if it unwound itself, that is dropped. Always returns
/// [`UNWIND`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_rethrow(context: *mut Context, held: Word) -> Word {
    unsafe {
        mask(context, false);
        abandon(context);
        let [code, token, args, trace] = *words(held) else {
            return fault(context, Fault::Cancelled);
        };
        (*context).fault = int(code).cast_unsigned();
        (*context).abort_token = if token == NULL { 0 } else { token };
        (*context).abort_args = 0;
        if args != NULL {
            amb_retain(args);
            (*context).abort_args = args;
        }
        amb_retain(trace);
        attach(context, trace);
        UNWIND
    }
}

/// Enters the cleanup of a block that completed. Returns null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_cleanup_enter(context: *mut Context) -> Word {
    unsafe { mask(context, true) };
    NULL
}

/// Leaves a cleanup that completed. Returns null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_cleanup_leave(context: *mut Context) -> Word {
    unsafe { mask(context, false) };
    NULL
}

/// Leaves a cleanup that unwound, after its block had completed. The
/// unwinding goes on, as a fault if it was an abort: nothing outside a
/// cleanup handles what it performs. Always returns [`UNWIND`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_cleanup_failed(context: *mut Context) -> Word {
    unsafe {
        mask(context, false);
        if (*context).fault == Fault::Abort as u64 {
            abandon(context);
            (*context).abort_token = 0;
            (*context).abort_args = 0;
            (*context).fault = Fault::CleanupAbort as u64;
        }
        UNWIND
    }
}

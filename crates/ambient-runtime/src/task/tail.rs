//! Tail calls: a function that ends with a call leaves before the call
//! is made, so a loop written as calls runs in constant stack. See
//! "Calls" in `spec/backends.md`.
//!
//! The function unwinds with the call pending in the context
//! ([`amb_tail`]), and whatever called it makes the call
//! ([`amb_bounce`]), again and again while what it calls ends the same
//! way. A host and the scheduler start a function the same way
//! ([`amb_start`]), so there is one place a function of any arity is
//! called from outside compiled code.

use alloc::vec;

use core::mem::transmute;

use super::{Frame, RESUMING, SUSPENDING, scheduler};
use crate::object::layout::{DEFINITION_CODE, DEFINITION_METERED};
use crate::object::{amb_release, amb_retain, fault, words};
use crate::{Context, Fault, NULL, UNWIND, Word};

/// The point of a frame that is not a function's: the call
/// [`amb_bounce`] was making when its task suspended.
const BOUNCE: u64 = u64::MAX;

/// Leaves the function that is running to call `callee` with `args`, a
/// tuple of the bindings it is handed and then its arguments, in its
/// metered form if `metered` is not zero. Always returns [`UNWIND`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_tail(
    context: *mut Context,
    callee: Word,
    args: Word,
    metered: u64,
) -> Word {
    unsafe {
        amb_retain(callee);
        amb_retain(args);
        let form = if metered == 0 {
            DEFINITION_CODE
        } else {
            DEFINITION_METERED
        };
        let definition = words(callee)[0] as usize as *const u8;
        (*context).tail = definition.add(form as usize).cast::<u64>().read();
        (*context).abort_token = callee;
        (*context).abort_args = args;
        fault(context, Fault::Tail)
    }
}

/// Whether the task is resuming into a call [`amb_bounce`] was making.
unsafe fn bounced(context: *mut Context) -> bool {
    unsafe {
        let frames = &scheduler(context).current().frames;
        (*context).mode == RESUMING && frames.last().is_some_and(|frame| frame.point == BOUNCE)
    }
}

/// Calls `callee` with `args`, as [`amb_tail`] takes them, from outside
/// compiled code: a host, or the scheduler. Called again with the same
/// once the task that suspended in it is to resume.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_start(
    context: *mut Context,
    callee: Word,
    args: Word,
    metered: u64,
) -> Word {
    unsafe {
        // A call that is kept is made again from where it is kept.
        if !bounced(context) {
            amb_tail(context, callee, args, metered);
        }
        amb_bounce(context)
    }
}

/// Called where a call unwound. If the unwinding is a function leaving for a
/// call, makes that call, and whatever calls it leaves for in turn, and
/// returns what the last gave. If not, returns [`UNWIND`] and the
/// unwinding goes on.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bounce(context: *mut Context) -> Word {
    unsafe {
        loop {
            let (code, callee, args) = if bounced(context) {
                let frame = scheduler(context).current().frames.pop();
                let Some(frame) = frame else { return UNWIND };
                (frame.position, frame.locals[0], frame.locals[1])
            } else if (*context).fault == Fault::Tail as u64 {
                (*context).fault = Fault::None as u64;
                let pending = (
                    (*context).tail,
                    (*context).abort_token,
                    (*context).abort_args,
                );
                ((*context).abort_token, (*context).abort_args) = (0, 0);
                pending
            } else {
                return UNWIND;
            };
            let result = call(context, code as usize, callee, words(args));
            if result == UNWIND && (*context).mode == SUSPENDING {
                // The call is kept, to be made again when the task
                // resumes: what it called has kept where it was.
                scheduler(context).current().frames.push(Frame {
                    point: BOUNCE,
                    definition: NULL,
                    position: code,
                    locals: vec![callee, args],
                });
                return UNWIND;
            }
            amb_release(callee);
            amb_release(args);
            if result != UNWIND {
                return result;
            }
        }
    }
}

/// A call of compiled code with the context, the closure, and the
/// words of `args` at each index listed.
macro_rules! call {
    ($code:expr, $context:expr, $callee:expr, $args:expr; $($index:literal)*) => {
        transmute::<
            usize,
            unsafe extern "C" fn(*mut Context, Word $(, call!(@word $index))*) -> Word,
        >($code)($context, $callee $(, $args[$index])*)
    };
    (@word $index:literal) => { Word };
}

/// Calls compiled code with as many arguments as `args` holds.
unsafe fn call(context: *mut Context, code: usize, callee: Word, args: &[Word]) -> Word {
    unsafe {
        match args.len() {
            1 => call!(code, context, callee, args; 0),
            2 => call!(code, context, callee, args; 0 1),
            3 => call!(code, context, callee, args; 0 1 2),
            4 => call!(code, context, callee, args; 0 1 2 3),
            5 => call!(code, context, callee, args; 0 1 2 3 4),
            6 => call!(code, context, callee, args; 0 1 2 3 4 5),
            7 => call!(code, context, callee, args; 0 1 2 3 4 5 6),
            8 => call!(code, context, callee, args; 0 1 2 3 4 5 6 7),
            9 => call!(code, context, callee, args; 0 1 2 3 4 5 6 7 8),
            10 => call!(code, context, callee, args; 0 1 2 3 4 5 6 7 8 9),
            11 => call!(code, context, callee, args; 0 1 2 3 4 5 6 7 8 9 10),
            12 => call!(code, context, callee, args; 0 1 2 3 4 5 6 7 8 9 10 11),
            13 => call!(code, context, callee, args; 0 1 2 3 4 5 6 7 8 9 10 11 12),
            14 => call!(code, context, callee, args; 0 1 2 3 4 5 6 7 8 9 10 11 12 13),
            15 => call!(code, context, callee, args; 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14),
            16 => call!(code, context, callee, args; 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15),
            // No function takes more: a backend refuses to compile one.
            _ => fault(context, Fault::Native),
        }
    }
}

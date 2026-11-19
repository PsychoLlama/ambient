//! Tail calls: a function that ends with a call leaves before the call
//! is made, so a loop written as calls runs in constant stack. See
//! "Calls" in `docs/spec/backends.md`.
//!
//! The function unwinds with the call pending in the context
//! ([`amb_tail`]), and whatever called it makes the call
//! ([`amb_bounce`]), again and again while what it calls ends the same
//! way. A host and the scheduler start a function the same way
//! ([`amb_start`]), so there is one place a function of any arity is
//! called from outside compiled code.

use alloc::vec;

use core::mem::transmute;

use super::depth::{BOUNCED, DEPTH, NOWHERE};
use super::{Frame, RESUMING, SUSPENDING, scheduler};
use crate::object::layout::DEFINITION_CODE;
use crate::object::{amb_release, amb_retain, fault, words};
use crate::{ARGUMENTS, Context, Fault, NULL, UNWIND, Word};

/// The point of a frame that is not a function's: the call
/// [`amb_bounce`] was making when its task suspended.
const BOUNCE: u64 = u64::MAX;

/// Leaves the function that is running to call `callee`. What it is called with, the
/// bindings it is handed and then its arguments, follows one at a time
/// through [`amb_tail_arg`]. Always returns [`UNWIND`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_tail(context: *mut Context, callee: Word) -> Word {
    unsafe {
        amb_retain(callee);
        let definition = words(callee)[0] as usize as *const u8;
        let code = definition.add(DEFINITION_CODE as usize).cast::<u64>();
        (*context).tail = code.read();
        (*context).abort_token = callee;
        (*context).tail_count = 0;
        fault(context, Fault::Tail)
    }
}

/// Adds `value` to what the pending call is made with. The context
/// keeps the arguments itself, so a loop written as calls allocates
/// nothing to go round.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_tail_arg(context: *mut Context, value: Word) {
    unsafe {
        let count = (*context).tail_count as usize;
        // No function takes more: a backend refuses to compile one.
        if count < ARGUMENTS {
            amb_retain(value);
            (*context).tail_args[count] = value;
            (*context).tail_count += 1;
        }
    }
}

/// A call waiting to be made: the code, the closure, and how many
/// arguments, each of which whoever takes it owns.
struct Pending {
    code: u64,
    callee: Word,
    count: usize,
}

impl Pending {
    /// The call the unwinding in `context` is for, if it is for one: the
    /// call a function left to make, or the one [`amb_bounce`] was
    /// making when its task suspended. Its arguments are put in `args`.
    unsafe fn take(context: *mut Context, args: &mut [Word; ARGUMENTS]) -> Option<Self> {
        unsafe {
            if bounced(context) {
                let frame = scheduler(context).current().frames.pop()?;
                let (callee, held) = frame.locals.split_first()?;
                args[..held.len()].copy_from_slice(held);
                Some(Self {
                    code: frame.position,
                    callee: *callee,
                    count: held.len(),
                })
            } else if (*context).fault == Fault::Tail as u64 {
                (*context).fault = Fault::None as u64;
                // Whatever is past the count is left from an earlier
                // call, and is not this one's.
                *args = (*context).tail_args;
                Some(Self {
                    code: (*context).tail,
                    callee: core::mem::take(&mut (*context).abort_token),
                    count: core::mem::take(&mut (*context).tail_count) as usize,
                })
            } else {
                None
            }
        }
    }
}

/// Whether the task is resuming into a call [`amb_bounce`] was making.
unsafe fn bounced(context: *mut Context) -> bool {
    unsafe {
        let frames = &scheduler(context).current().frames;
        (*context).mode == RESUMING && frames.last().is_some_and(|frame| frame.point == BOUNCE)
    }
}

/// Calls `callee` with the members of `args`, a tuple of the bindings
/// it is handed and then its arguments, from outside compiled code: a
/// host, or the scheduler. Called again with the same once the task
/// that suspended in it is to resume.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_start(context: *mut Context, callee: Word, args: Word) -> Word {
    unsafe {
        if let Some(stacks) = super::stack::of(context) {
            return super::stack::root(context, stacks, callee, args);
        }
        // Nothing compiled is running under it: a host is, or the
        // scheduler a host called.
        start(context, callee, args, NOWHERE)
    }
}

/// What [`amb_start`] does, from `depth`: how deep whatever makes the
/// call is.
pub(super) unsafe fn start(context: *mut Context, callee: Word, args: Word, depth: Word) -> Word {
    unsafe {
        // A call that is kept is made again from where it is kept.
        if !bounced(context) {
            amb_tail(context, callee);
            for arg in words(args).iter() {
                amb_tail_arg(context, *arg);
            }
        }
        amb_bounce(context, depth)
    }
}

/// Called where a call unwound. If the unwinding is a function leaving for a
/// call, makes that call, and whatever calls it leaves for in turn, and
/// returns what the last gave. If not, returns [`UNWIND`] and the
/// unwinding goes on.
///
/// `depth` is how deep the function the call was made from is, as
/// [`super::amb_enter`] gave it: each call it makes starts from there,
/// whatever the one before it reached.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bounce(context: *mut Context, depth: Word) -> Word {
    unsafe {
        // What the call is made with is copied out of the context,
        // since what is called may leave for a call of its own.
        let mut args = [NULL; ARGUMENTS];
        while let Some(Pending {
            code,
            callee,
            count,
        }) = Pending::take(context, &mut args)
        {
            let args = &args[..count];
            // What is called stands on this, which counts as a call.
            let beneath = (depth >> 1).saturating_add(BOUNCED);
            let result = if beneath <= DEPTH {
                (*context).depth = beneath;
                call(context, code as usize, callee, args)
            } else {
                fault(context, Fault::Deep)
            };
            if result == UNWIND && (*context).mode == SUSPENDING {
                // The call is kept, to be made again when the task
                // resumes: what it called has kept where it was.
                let mut locals = vec![callee];
                for arg in args {
                    locals.push(*arg);
                }
                scheduler(context).current().frames.push(Frame {
                    point: BOUNCE,
                    position: code,
                    locals,
                });
                return UNWIND;
            }
            amb_release(callee);
            for arg in args {
                amb_release(*arg);
            }
            if result != UNWIND {
                return result;
            }
        }
        UNWIND
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

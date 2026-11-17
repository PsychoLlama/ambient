//! Calls native code makes back into its program. See "Calling back" in
//! `spec/externs.md`.
//!
//! Native code is on the stack while what it calls runs, and nothing
//! can save it there. So a task that suspends beneath native code is
//! not unwound past it: the call back gives other tasks their turns
//! where it stands ([`amb_call_among`]) until the task can go on, and
//! calls again to resume it. Only what the world would have to answer
//! cannot be waited for, since a host hands answers over from outside
//! every call; a task that can go on no other way unwinds with
//! [`Fault::Pinned`].

use super::depth::NATIVE;
use super::tail::start;
use super::{Beneath, ROOT, RUNNING, SUSPENDING, scheduler, turn};
use crate::object::{fault, small};
use crate::{Context, DEPTH, Fault, UNWIND, Word};

/// How deep what native code calls starts from: where the function
/// that called the native code is, and what native code is taken to
/// keep. None if that is deeper than calls may nest.
unsafe fn beneath(context: *mut Context) -> Option<Word> {
    let depth = unsafe { (*context).depth }.saturating_add(NATIVE);
    small(depth.cast_signed()).filter(|_| depth <= DEPTH)
}

/// Whether the task that is running is inside a budget, and so calls
/// the metered form of what it calls.
unsafe fn metered(context: *mut Context) -> u64 {
    u64::from(unsafe { scheduler(context) }.current().budget.is_some())
}

/// Calls `callee` for native code, with the members of `args`: a tuple
/// of the bindings it is handed and then its arguments. For a program
/// in which nothing suspends; any other is called through
/// [`amb_call_among`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_call_back(context: *mut Context, callee: Word, args: Word) -> Word {
    unsafe {
        let outer = (*context).depth;
        let Some(depth) = beneath(context) else {
            return fault(context, Fault::Deep);
        };
        let result = start(context, callee, args, metered(context), depth);
        (*context).depth = outer;
        result
    }
}

/// Calls `callee` for native code, as [`amb_call_back`] does, in a
/// program whose tasks can suspend. If the task suspends in the call,
/// others take their turns here until it can go on. Where a task is a
/// stack the call never comes back suspending: the wait it is in gives
/// the turns ([`super::waiting`]), from here all the same.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_call_among(context: *mut Context, callee: Word, args: Word) -> Word {
    unsafe {
        let outer = (*context).depth;
        let Some(depth) = beneath(context) else {
            return fault(context, Fault::Deep);
        };
        let metered = metered(context);
        let scheduler = scheduler(context);
        let id = scheduler.current;
        // Native code that may answer later may still do so once what
        // it called is over, whatever that called in turn.
        let open = core::mem::take(&mut scheduler.waits.open);
        // It may be beneath native code already, and even unwinding to
        // it: a cleanup may call native code too.
        let before = scheduler.current().beneath;
        if before == Beneath::Free {
            scheduler.current().beneath = Beneath::Pinned;
        }
        let stood = scheduler.current().kept.stands(depth);
        let result = loop {
            let result = start(context, callee, args, metered, depth);
            if result != UNWIND || (*context).mode != SUSPENDING {
                break result;
            }
            (*context).mode = RUNNING;
            until(context, id, depth);
        };
        let scheduler = super::scheduler(context);
        scheduler.waits.open = open;
        let task = scheduler.task(id);
        // It was made to unwind because it could not go on. That is a
        // fault, unless it was to unwind anyway.
        let after = core::mem::replace(&mut task.beneath, before);
        task.kept.stands(stood);
        let stuck = after == Beneath::Stuck && before != Beneath::Stuck;
        let cancelled = task.cancelled || task.disturbed > 0;
        if stuck && !cancelled && result == UNWIND && (*context).fault == Fault::Cancelled as u64 {
            (*context).fault = Fault::Pinned as u64;
        }
        (*context).depth = outer;
        result
    }
}

/// Gives tasks their turns until the task `id`, which suspended beneath
/// native code, can go on, and sets the mode so that calling again
/// resumes it. Each starts from `depth`, since each runs above the
/// native code.
///
/// A task that is itself beneath native code further down the stack
/// cannot have a turn here, and neither can the root, which only the
/// host resumes. If nothing else can run, `id` waits on what will not
/// happen while native code stands: it is made to unwind.
pub(super) unsafe fn until(context: *mut Context, id: usize, depth: Word) {
    unsafe {
        loop {
            let scheduler = scheduler(context);
            let here = |task: usize| {
                let free = |task: &super::Task| task.beneath == Beneath::Free;
                task == id || (task != ROOT && scheduler.tasks[task].as_ref().is_some_and(free))
            };
            let next = scheduler.ready.iter().position(|task| here(*task));
            let next = next.and_then(|at| scheduler.ready.remove(at));
            let next = next.unwrap_or_else(|| {
                scheduler.task(id).beneath = Beneath::Stuck;
                id
            });
            if next != id {
                turn(context, next, depth);
                continue;
            }
            return scheduler.resumes(context, id);
        }
    }
}

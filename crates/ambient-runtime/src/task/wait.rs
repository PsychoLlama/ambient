//! Waiting on the host: a task suspended in native code that will
//! answer later. See "Native side" in `spec/externs.md`.
//!
//! Native code that cannot answer at once asks for a ticket
//! ([`amb_await`]) and returns. The task suspends, and the host hands
//! the answer over by its ticket when it next calls in ([`amb_answer`],
//! [`amb_refuse`]); the task then arrives back at the call it left,
//! which gives the answer as its result.

use super::{WAITING, amb_suspend, arriving, scheduler};
use crate::native::amb_native;
use crate::object::{amb_release, fault};
use crate::{Context, Fault, Word};

/// The index native code is called with to be told a task left a wait:
/// no extern has it. The ticket is passed where the bindings are, and
/// what the call returns is not looked at.
pub const FORSAKEN: u64 = u64::MAX;

/// Where a task is in waiting on the host.
#[derive(Default)]
pub(super) enum Wait {
    #[default]
    None,
    /// It waits for the answer to this ticket.
    Pending(u64),
    /// The host answered: a value the task owns, or nothing if what it
    /// asked for failed.
    Answered(Option<Word>),
}

/// The waits of a program's tasks on its host.
#[derive(Default)]
pub(super) struct Waits {
    /// The last ticket given out. None is zero.
    tickets: u64,
    /// How many tasks wait for an answer.
    pending: usize,
    /// Whether the host said no answer is coming.
    stranded: bool,
    /// Whether the native code running now may answer later.
    pub(super) open: bool,
}

impl super::Scheduler {
    /// Whether the host is to be waited for: nothing can run, and an
    /// answer may still wake a task.
    pub(super) fn awaits(&self) -> bool {
        self.ready.is_empty() && self.waits.pending > 0 && !self.waits.stranded
    }

    /// Takes the current task out of its wait, if it is in one. Gives
    /// the ticket of a wait that had no answer yet.
    unsafe fn leave(&mut self) -> Option<u64> {
        match core::mem::take(&mut self.current().wait) {
            Wait::None => None,
            Wait::Pending(ticket) => {
                self.waits.pending -= 1;
                Some(ticket)
            }
            Wait::Answered(answer) => {
                unsafe { amb_release(answer.unwrap_or(crate::NULL)) };
                None
            }
        }
    }

    /// Gives the task that waits on `ticket` its answer and its turn.
    /// Whether one did.
    fn answer(&mut self, ticket: u64, answer: Option<Word>) -> bool {
        let waiting = |task: &Option<super::Task>| matches!(task, Some(task) if matches!(task.wait, Wait::Pending(pending) if pending == ticket));
        let Some(id) = self.tasks.iter().position(waiting) else {
            return false;
        };
        self.task(id).wait = Wait::Answered(answer);
        self.waits.pending -= 1;
        self.queue(id);
        true
    }
}

/// Runs the extern at `index`, which may answer later: see
/// [`crate::amb_native`] for what it is handed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_native_wait(
    context: *mut Context,
    index: u64,
    bindings: Word,
    args: Word,
) -> Word {
    unsafe { super::waiting(context, || asked(context, index, bindings, args)) }
}

/// A step of a wait on the host: running the native code, or arriving
/// back with its answer.
unsafe fn asked(context: *mut Context, index: u64, bindings: Word, args: Word) -> Word {
    unsafe {
        let arriving = arriving(context);
        // Arriving or not, a cancelled task unwinds from here.
        if scheduler(context).interrupted() {
            // The host is told at once, so it keeps no list of waits
            // to ask about, and the runtime none to be asked.
            if let Some(ticket) = scheduler(context).leave() {
                amb_native(context, FORSAKEN, ticket, crate::NULL);
            }
            return fault(context, Fault::Cancelled);
        }
        if arriving {
            let task = scheduler(context).current();
            match core::mem::take(&mut task.wait) {
                Wait::Answered(Some(value)) => return value,
                Wait::Answered(None) => return fault(context, Fault::Native),
                // It was given a turn for another reason, and a cleanup
                // holds cancellation off: it goes on waiting.
                Wait::Pending(ticket) => {
                    task.wait = Wait::Pending(ticket);
                    return amb_suspend(context);
                }
                Wait::None => {}
            }
        }
        scheduler(context).waits.open = true;
        let result = amb_native(context, index, bindings, args);
        scheduler(context).waits.open = false;
        if matches!(scheduler(context).current().wait, Wait::Pending(_)) {
            return amb_suspend(context);
        }
        result
    }
}

/// For native code that will answer later: makes the task that called
/// it wait, and gives the ticket its answer is to come by. What the
/// native code then returns is not looked at.
///
/// Gives zero, and nothing waits, if the extern that is running was
/// not declared to suspend.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_await(context: *mut Context) -> u64 {
    let scheduler = unsafe { scheduler(context) };
    if !scheduler.waits.open {
        return 0;
    }
    scheduler.waits.open = false;
    scheduler.waits.tickets += 1;
    scheduler.waits.pending += 1;
    let ticket = scheduler.waits.tickets;
    scheduler.current().wait = Wait::Pending(ticket);
    ticket
}

/// Answers a wait with `value`, which the task takes. An answer to a
/// wait its task has left is dropped.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_answer(context: *mut Context, ticket: u64, value: Word) {
    unsafe {
        if !scheduler(context).answer(ticket, Some(value)) {
            amb_release(value);
        }
    }
}

/// Answers a wait with a failure: its task faults as it would if native
/// code had failed at once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_refuse(context: *mut Context, ticket: u64) {
    unsafe { scheduler(context) }.answer(ticket, None);
}

/// Says no answer is coming to any wait: [`crate::amb_schedule`] then
/// treats tasks that wait on the host as waiting for nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_strand(context: *mut Context) {
    unsafe { scheduler(context) }.waits.stranded = true;
}

/// Leaves the mode a host reads as "wait for an answer, hand it over,
/// and schedule again".
pub(super) unsafe fn idle(context: *mut Context) {
    unsafe { (*context).mode = WAITING };
}

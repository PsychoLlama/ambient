//! Capturing a trace: where a task is, and the calls it is in. See
//! `spec/errors.md`.
//!
//! Nothing keeps a record of calls as they are made, so a task that asks
//! for its trace suspends: every function it is in saves a frame, which
//! says where the function was, and the task is resumed at once. Only a
//! capture pays for a capture.

use super::{Capture, Scheduler, arriving, scheduler, suspend};
use crate::trace::{TRACE_LIMIT, frames};
use crate::{Context, NULL, Word};

impl Scheduler {
    /// Captures the trace of a task that suspended for it: its frames,
    /// innermost first, and then those of what started it.
    pub(super) unsafe fn capture(&mut self, id: usize) {
        if !matches!(self.task(id).capture, Capture::Asked) {
            return;
        }
        let mut found = Vec::new();
        let mut next = Some(id);
        while let Some(task) = next.and_then(|id| self.tasks[id].as_ref()) {
            let placed = task.frames.iter().filter(|frame| frame.definition != NULL);
            found.extend(placed.map(|frame| (frame.definition, frame.position)));
            let join = task.place.and_then(|(join, _)| self.joins[join].as_ref());
            next = join.map(|join| join.parent);
        }
        found.truncate(TRACE_LIMIT);
        self.task(id).capture = Capture::Ready(unsafe { frames(&found) });
    }
}

/// The trace of the current task where it is: a frame for this call and
/// for each call it is inside, nearest first, as the list an error holds.
/// The caller owns it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_capture(context: *mut Context) -> Word {
    unsafe {
        let arriving = arriving(context);
        let scheduler = scheduler(context);
        if arriving {
            return match std::mem::take(&mut scheduler.current().capture) {
                Capture::Ready(trace) => trace,
                _ => NULL,
            };
        }
        // The task goes first among those waiting their turn: nothing
        // runs between its suspending and its coming back.
        let id = scheduler.current;
        scheduler.ready.retain(|waiting| *waiting != id);
        scheduler.ready.push_front(id);
        let task = scheduler.current();
        task.queued = true;
        task.capture = Capture::Asked;
        suspend(context)
    }
}

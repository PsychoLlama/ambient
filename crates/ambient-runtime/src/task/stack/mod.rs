//! A task as a stack. See "Suspension" in `spec/backends.md`.
//!
//! Where a platform can keep a stack and come back to it, a task that
//! suspends is left as it stands: nothing it is inside of unwinds. The
//! platform says so by putting [`Stacks`] in the context, which is all
//! the runtime asks of it.
//!
//! The scheduler is the same either way. A wait is written once, as a
//! step that says the task is suspending and is asked again when the
//! task has its turn ([`waiting`]). Unwinding carries that answer out
//! through every function of the task and the turn calls them all
//! again; here the task parks inside the wait and the turn goes on
//! with its stack. Which task runs when is decided by the same code, so
//! a program behaves the same however its tasks are kept.
//!
//! WebAssembly keeps no stack, and its runtime is built without any of
//! this ([`none`]): a program there pays nothing for what it cannot do.

#[cfg(not(target_arch = "wasm32"))]
mod kept;
#[cfg(target_arch = "wasm32")]
mod none;

#[cfg(not(target_arch = "wasm32"))]
use kept as platform;
#[cfg(target_arch = "wasm32")]
use none as platform;

pub use platform::{Body, Stacks};
pub(super) use platform::{Kept, of, root, turn};

use super::back::until;
use super::{Beneath, RUNNING, SUSPENDING, scheduler};
use crate::{Context, UNWIND, Word};

/// A wait: `step` says what the task is to do, and is asked again each
/// time the task has had to wait and is given its turn.
///
/// A step that unwinds with the mode [`SUSPENDING`] says the task
/// waits. Where tasks unwind that is the answer, and the turn comes
/// back here by calling everything again. Where a task is a stack it
/// waits here.
pub(crate) unsafe fn waiting(context: *mut Context, mut step: impl FnMut() -> Word) -> Word {
    unsafe {
        loop {
            let result = step();
            let Some(stacks) = of(context) else {
                return result;
            };
            if result != UNWIND || (*context).mode != SUSPENDING {
                return result;
            }
            let scheduler = scheduler(context);
            let id = scheduler.current;
            let task = scheduler.current();
            if task.beneath == Beneath::Free {
                platform::park(context, stacks);
                continue;
            }
            // Native code it called stands beneath it, so others take
            // their turns here and not from outside: see `back`.
            let depth = task.kept.stood();
            (*context).mode = RUNNING;
            until(context, id, depth);
        }
    }
}

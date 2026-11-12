//! Groups: a block whose body starts tasks and which ends only once they
//! all have. See `spec/tasks.md`.
//!
//! A group is a join whose starter does not wait at once: it runs the
//! group's body, which spawns into the join, and waits when the body is
//! over. If a task fails first, the starter is disturbed: it unwinds from
//! its next suspension in the body, as if cancelled, and the group goes on
//! unwinding with the task's failure.

use super::{Join, Task, arriving, interrupted, mask, scheduler, suspend};
use crate::abort::abandon;
use crate::number::amb_int_new;
use crate::object::{amb_retain, fault, int};
use crate::{Context, Fault, NULL, UNWIND, Word, amb_rethrow};

/// A group with no tasks yet, started by the current task.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_group_new(context: *mut Context) -> Word {
    unsafe {
        let scheduler = scheduler(context);
        let id = scheduler.joins.len();
        scheduler.joins.push(Some(Join {
            parent: scheduler.current,
            children: Vec::new(),
            results: Vec::new(),
            remaining: 0,
            failure: None,
            first: false,
            winner: None,
            group: true,
            closing: false,
        }));
        amb_int_new(id as i64)
    }
}

/// Starts `thunk`, a closure that takes nothing, as a task of `group`.
/// Returns null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_spawn(context: *mut Context, group: Word, thunk: Word) -> Word {
    unsafe {
        let scheduler = scheduler(context);
        let join = int(group) as usize;
        let id = scheduler.tasks.len();
        let Some(Some(members)) = scheduler.joins.get_mut(join) else {
            return NULL;
        };
        let place = members.children.len();
        members.children.push(id);
        members.remaining += 1;
        // A group that is already stopping starts nothing more.
        let cancelled = members.closing || members.failure.is_some();
        let budget = scheduler.current().budget;
        amb_retain(thunk);
        scheduler.tasks.push(Some(Task {
            thunk,
            place: Some((join, place)),
            cancelled,
            budget,
            ..Task::default()
        }));
        scheduler.queue(id);
        NULL
    }
}

/// Waits for every task of `group` to be over, and returns null. With
/// `unwinding`, or if the starter is to unwind, or if a task failed, the
/// tasks are cancelled first.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_group_close(
    context: *mut Context,
    group: Word,
    unwinding: u64,
) -> Word {
    unsafe {
        arriving(context);
        let stop = unwinding != 0 || interrupted(context);
        let scheduler = scheduler(context);
        let Some(Some(join)) = scheduler.joins.get_mut(int(group) as usize) else {
            return NULL;
        };
        let waiting = join.remaining > 0;
        if (stop || join.failure.is_some()) && !join.closing {
            join.closing = true;
            for child in join.children.clone() {
                if scheduler.tasks[child].is_some() {
                    scheduler.cancel(child);
                }
            }
        }
        if waiting { suspend(context) } else { NULL }
    }
}

/// Ends `group`, whose tasks are over. `held` is the unwinding its body
/// left by, or null. Returns null if the group completed. If a task
/// failed, unwinds as that task did; if not, goes on with `held`, or
/// with the starter's own cancellation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_group_end(context: *mut Context, group: Word, held: Word) -> Word {
    unsafe {
        let scheduler = scheduler(context);
        let failure = scheduler
            .joins
            .get_mut(int(group) as usize)
            .and_then(Option::take)
            .and_then(|join| join.failure);
        let Some(failure) = failure else {
            if held != NULL {
                return amb_rethrow(context, held);
            }
            if interrupted(context) {
                return fault(context, Fault::Cancelled);
            }
            return NULL;
        };
        // The failure came first: whatever the body left by came of it.
        let task = scheduler.current();
        task.disturbed = task.disturbed.saturating_sub(1);
        if held != NULL {
            mask(context, false);
        }
        abandon(context);
        (*context).fault = failure.fault;
        (*context).abort_token = failure.token;
        (*context).abort_args = failure.args;
        UNWIND
    }
}

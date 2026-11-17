//! Joining: tasks started together, and their starter waiting for them.
//! See `spec/tasks.md`.

use alloc::vec;
use alloc::vec::Vec;

use super::{Gives, Join, Task, amb_suspend, arriving, interrupted, scheduler};
use crate::list::members;
use crate::object::{KIND_LIST, amb_release, amb_retain, fault, kind};
use crate::sequence::{amb_sequence_new, amb_sequence_set};
use crate::{Context, Fault, NULL, UNWIND, Word};

/// Runs each of `thunks`, a tuple or a list of closures that take
/// nothing, as a task, and gives their results once every one is over:
/// a tuple for a tuple, and a list for a list.
///
/// If one unwinds, the others are cancelled, and once they are over the
/// unwinding goes on from here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_all(context: *mut Context, thunks: Word) -> Word {
    unsafe { join(context, thunks, false) }
}

/// Runs each of `thunks` as [`amb_all`] does, and gives the result of the
/// first that is over once every one is: the rest are cancelled.
///
/// With no thunks there is never a first, so the task waits until it is
/// cancelled.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_first(context: *mut Context, thunks: Word) -> Word {
    unsafe { join(context, thunks, true) }
}

/// Starts a task for each of `thunks` and waits for them all.
unsafe fn join(context: *mut Context, thunks: Word, first: bool) -> Word {
    unsafe { super::waiting(context, || started(context, thunks, first)) }
}

/// A step of a join: starting its tasks, or arriving back at it.
unsafe fn started(context: *mut Context, thunks: Word, first: bool) -> Word {
    unsafe {
        if arriving(context) {
            return joined(context);
        }
        if interrupted(context) {
            return fault(context, Fault::Cancelled);
        }
        let scheduler = scheduler(context);
        let parent = scheduler.current;
        let gives = match (first, kind(thunks) == KIND_LIST) {
            (true, _) => Gives::First,
            (false, true) => Gives::List,
            (false, false) => Gives::Tuple,
        };
        let thunks = members(thunks);
        let budget = scheduler.task(parent).budget;
        let join = scheduler.open(Join {
            parent,
            children: Vec::new(),
            results: vec![NULL; thunks.len()],
            remaining: thunks.len(),
            failure: None,
            gives,
            winner: None,
            closing: false,
        });
        let mut children = Vec::new();
        for (place, thunk) in thunks.iter().enumerate() {
            amb_retain(*thunk);
            let id = scheduler.admit(Task {
                thunk: *thunk,
                place: Some((join, place)),
                budget: super::budget::shared(budget),
                ..Task::default()
            });
            scheduler.queue(id);
            children.push(id);
        }
        if let Some(Some(join)) = scheduler.joins.get_mut(join) {
            join.children = children;
        }
        scheduler.current().join = Some(join);
        if thunks.is_empty() && !first {
            scheduler.queue(parent);
        }
        amb_suspend(context)
    }
}

/// A task arrives back at the join it was waiting on.
unsafe fn joined(context: *mut Context) -> Word {
    unsafe {
        let scheduler = scheduler(context);
        let Some(id) = scheduler.current().join else {
            return fault(context, Fault::Cancelled);
        };
        let unfinished = scheduler.joins[id]
            .as_ref()
            .is_some_and(|join| join.remaining > 0);
        if unfinished && !scheduler.interrupted() {
            // It was cancelled inside a cleanup, which goes on waiting.
            return amb_suspend(context);
        }
        if unfinished {
            // Only cancellation wakes a task whose tasks are not over. It
            // cannot leave before they have, so it passes it on and waits.
            let children = scheduler.joins[id]
                .as_ref()
                .map(|join| join.children.clone())
                .unwrap_or_default();
            for child in children {
                if scheduler.live(child) {
                    scheduler.cancel(child);
                }
            }
            return amb_suspend(context);
        }
        scheduler.current().join = None;
        let Some(join) = scheduler.close(id) else {
            return fault(context, Fault::Cancelled);
        };
        let interrupted = scheduler.interrupted();
        let failure = match join.failure {
            Some(failure) if !interrupted => {
                (*context).abort_token = failure.token;
                (*context).abort_args = failure.args;
                crate::trace::attach(context, failure.trace);
                Some(failure.fault)
            }
            // Its own cancellation comes before what its tasks did.
            failure => {
                if let Some(failure) = failure {
                    failure.drop();
                }
                interrupted.then_some(Fault::Cancelled as u64)
            }
        };
        if let Some(code) = failure {
            join.results.iter().for_each(|result| amb_release(*result));
            (*context).fault = code;
            return UNWIND;
        }
        if join.gives == Gives::First {
            let winner = join.winner.unwrap_or_default();
            for (place, result) in join.results.iter().enumerate() {
                if place != winner {
                    amb_release(*result);
                }
            }
            return join.results.get(winner).copied().unwrap_or(NULL);
        }
        let tuple = amb_sequence_new(
            u64::from(join.gives == Gives::List),
            join.results.len() as u64,
        );
        for (index, result) in join.results.iter().enumerate() {
            amb_sequence_set(tuple, index as u64, *result);
            amb_release(*result);
        }
        tuple
    }
}

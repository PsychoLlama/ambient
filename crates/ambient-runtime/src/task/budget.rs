//! Budgets: a bound on the steps and the allocation of whatever runs
//! inside one. See `spec/budgets.md`.
//!
//! Metered code charges as it goes. A charge counts against every budget
//! the task is inside, and the first to run out unwinds the task to it.
//! Tasks started inside a budget are inside it too.

use super::{Scheduler, Task, scheduler};
use crate::object::{
    KIND_BUDGET, KIND_LIST, KIND_RECORD, amb_release, amb_retain, fault, header, int, is_object,
    kind, object, words,
};
use crate::record;
use crate::{Context, Fault, NULL, UNWIND, Word};

/// The limits of a budget, as the tags that say which ran out: of steps,
/// and of allocation.
pub const LIMITS: [&str; 2] = ["steps", "allocation"];

/// A budget is an object, counted as a value is, though no program
/// sees one: its body's function holds it, and so does each task
/// inside it, each budget entered inside it, and each handler bound
/// inside it. Its words are these.
///
/// The budget it was entered inside, or null: the one word that is
/// counted.
const PARENT: usize = 0;
/// What is left of each limit.
const STEPS: usize = 1;
const ROOM: usize = 2;
/// Which of [`LIMITS`] ran out, and one; zero until one has.
const EXHAUSTED: usize = 3;
/// Whether its body is still running.
const LIVE: usize = 4;

/// What a budget that ran out allows besides, in steps and in words,
/// for the cleanups it unwinds through. It is small and fixed, so a
/// budget still bounds what runs inside it.
const GRACE: u64 = 1_000;

/// The budget `budget` was entered inside.
unsafe fn parent(budget: Word) -> Option<Word> {
    let parent = unsafe { words(budget)[PARENT] };
    (parent != NULL).then_some(parent)
}

/// Another reference to a budget, if there is one.
pub(super) unsafe fn shared(budget: Option<Word>) -> Option<Word> {
    unsafe { amb_retain(budget.unwrap_or(NULL)) };
    budget
}

/// The limit `budget` ran out of, if it did.
unsafe fn exhausted(budget: Word) -> Option<&'static str> {
    let limit = unsafe { words(budget)[EXHAUSTED] } as usize;
    LIMITS.get(limit.wrapping_sub(1)).copied()
}

/// Whether `budget` is `innermost` or a budget around it.
unsafe fn around(innermost: Option<Word>, budget: Word) -> bool {
    let mut next = innermost;
    while let Some(inner) = next {
        if inner == budget {
            return true;
        }
        next = unsafe { parent(inner) };
    }
    false
}

/// Takes `steps` and `room` from `budget` for a task that `told` made
/// unwind, if any did, and says whether the task is to unwind: the
/// budget cannot afford them, or ran out in another task and this one
/// has yet to hear.
unsafe fn charge(budget: Word, told: &mut Option<Word>, steps: u64, room: u64) -> bool {
    unsafe {
        let fields = words(budget);
        let short = [fields[STEPS] < steps, fields[ROOM] < room];
        if fields[EXHAUSTED] == 0 {
            let Some(limit) = short.iter().position(|short| *short) else {
                fields[STEPS] -= steps;
                fields[ROOM] -= room;
                return false;
            };
            fields[EXHAUSTED] = limit as u64 + 1;
            (fields[STEPS], fields[ROOM]) = (GRACE, GRACE);
        } else if told.is_some_and(|told| around(Some(budget), told)) {
            // A cleanup, drawing on what the budget allows for one.
            fields[STEPS] = fields[STEPS].saturating_sub(steps);
            fields[ROOM] = fields[ROOM].saturating_sub(room);
            return short.contains(&true);
        }
        // It is around whichever told the task before.
        amb_release(core::mem::replace(told, shared(Some(budget))).unwrap_or(NULL));
        true
    }
}

impl Task {
    /// Puts the task inside `budget`, or none. Gives the one it was
    /// inside, which is the caller's to release.
    unsafe fn inside(&mut self, budget: Option<Word>) -> Option<Word> {
        unsafe {
            // What a budget told it matters only while it is inside.
            if let Some(told) = self.told
                && !around(budget, told)
            {
                amb_release(told);
                self.told = None;
            }
            core::mem::replace(&mut self.budget, shared(budget))
        }
    }

    /// Gives up the budgets a task that is over was inside or told by.
    pub(super) unsafe fn leaves(&mut self) {
        unsafe { amb_release(self.inside(None).unwrap_or(NULL)) };
    }
}

impl Scheduler {
    /// Puts the current task inside `budget`, and drops the reference
    /// to the one it was inside.
    unsafe fn within(&mut self, budget: Option<Word>) {
        unsafe { amb_release(self.current().inside(budget).unwrap_or(NULL)) };
    }
}

/// Enters a budget of `steps` steps and `words` words of allocation, both
/// integers; a negative limit allows nothing. Returns the budget, for the
/// entry points that end it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_budget_enter(context: *mut Context, steps: Word, words: Word) -> Word {
    unsafe {
        let limit = |word| u64::try_from(int(word)).unwrap_or(0);
        let budget = object(KIND_BUDGET, 40);
        let task = scheduler(context).current();
        // The task's reference to the one around becomes the budget's.
        let around = task.budget.take().unwrap_or(NULL);
        let fields = crate::object::words(budget);
        (fields[PARENT], fields[STEPS], fields[ROOM]) = (around, limit(steps), limit(words));
        (fields[EXHAUSTED], fields[LIVE]) = (0, 1);
        task.budget = shared(Some(budget));
        budget
    }
}

/// Leaves `budget`, whose body is over. Returns null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_budget_leave(context: *mut Context, budget: Word) -> Word {
    unsafe {
        words(budget)[LIVE] = 0;
        scheduler(context).within(parent(budget));
        NULL
    }
}

/// Called by a budget that its body unwound through. Leaves it. If it ran
/// out, ends the unwinding and returns the limit that did, as a tag; if
/// not, returns [`UNWIND`] and the unwinding goes on.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_budget_caught(context: *mut Context, budget: Word) -> Word {
    unsafe {
        amb_budget_leave(context, budget);
        let Some(limit) = exhausted(budget) else {
            return UNWIND;
        };
        if (*context).fault != Fault::Exhausted as u64 {
            return UNWIND;
        }
        (*context).fault = Fault::None as u64;
        crate::boundary::tag(limit)
    }
}

/// Charges `steps` steps and `words` words of allocation to every budget
/// the task is inside. Returns null, or unwinds: because one ran out, or
/// because the task was cancelled, which metered code notices here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_charge(context: *mut Context, steps: u64, words: u64) -> Word {
    unsafe {
        let scheduler = scheduler(context);
        let mut next = scheduler.current().budget;
        if next.is_none() {
            return NULL;
        }
        if scheduler.interrupted() {
            return fault(context, Fault::Cancelled);
        }
        let told = &mut scheduler.current().told;
        let mut spent = false;
        while let Some(budget) = next {
            spent |= charge(budget, told, steps, words);
            next = parent(budget);
        }
        if spent {
            return fault(context, Fault::Exhausted);
        }
        NULL
    }
}

/// The budget the task is inside, for [`amb_budget_switch`], or null.
/// A provide keeps this beside each handler it binds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_budget_current(context: *mut Context) -> Word {
    unsafe { shared(scheduler(context).current().budget).unwrap_or(NULL) }
}

/// Puts the task inside `budget`, as [`amb_budget_current`] gave it, for
/// as long as a handler bound there runs. If that budget is over, the
/// nearest around it that is not stands in. Returns the budget the task
/// was inside, for [`amb_budget_back`], which the caller then releases.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_budget_switch(context: *mut Context, budget: Word) -> Word {
    unsafe {
        let mut to = (budget != NULL).then_some(budget);
        while let Some(over) = to
            && words(over)[LIVE] == 0
        {
            to = parent(over);
        }
        scheduler(context).current().inside(to).unwrap_or(NULL)
    }
}

/// Puts the task back inside the budget [`amb_budget_switch`] took it out
/// of. Returns null, or, with `unwinding`, [`UNWIND`]: the handler
/// unwound, and that goes on.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_budget_back(
    context: *mut Context,
    saved: Word,
    unwinding: u64,
) -> Word {
    unsafe {
        scheduler(context).within((saved != NULL).then_some(saved));
        if unwinding == 0 { NULL } else { UNWIND }
    }
}

/// Charges for a value about as large as `value`: what an operation that
/// copies it allocates.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_charge_size(context: *mut Context, value: Word) -> Word {
    unsafe {
        let words = if !is_object(value) {
            0
        } else if kind(value) == KIND_RECORD {
            record::copying(value)
        } else if kind(value) == KIND_LIST {
            // The charge ignores sharing: a list counts for its members
            // wherever they are kept.
            crate::list::members(value).len() as u64 + 1
        } else {
            (*header(value)).size.div_ceil(8) + 1
        };
        amb_charge(context, 0, words)
    }
}

/// Charges for the tasks an all or a first over `thunks`, a list,
/// starts: a step for each, and the list of what they give.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_charge_tasks(context: *mut Context, thunks: Word) -> Word {
    unsafe {
        let count = crate::list::members(thunks).len() as u64;
        amb_charge(context, count, count + 1)
    }
}

/// Charges for what reading `key` of `record` allocates: a function,
/// if the record holds a method there.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_charge_read(context: *mut Context, record: Word, key: Word) -> Word {
    unsafe {
        let held = record::find(record, key).unwrap_or(NULL);
        amb_charge(context, 0, record::binding(held))
    }
}

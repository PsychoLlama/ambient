//! Budgets: a bound on the steps and the allocation of whatever runs
//! inside one. See `spec/budgets.md`.
//!
//! Metered code charges as it goes. A charge counts against every budget
//! the task is inside, and the first to run out unwinds the task to it.
//! Tasks started inside a budget are inside it too.

use super::scheduler;
use crate::boundary::FAULT_PACKAGE;
use crate::number::amb_int_new;
use crate::object::{fault, header, int, is_object};
use crate::text::amb_tag;
use crate::{Context, Fault, NULL, UNWIND, Word};

/// The limits of a budget, as the tags that say which ran out: of steps,
/// and of allocation.
pub const LIMITS: [&str; 2] = ["steps", "allocation"];

pub(super) struct Budget {
    /// What is left of each limit.
    steps: u64,
    words: u64,
    /// The budget this one was entered inside.
    parent: Option<usize>,
    /// The limit that ran out, once one has.
    exhausted: Option<&'static str>,
    /// Whether its body is still running.
    live: bool,
}

/// Enters a budget of `steps` steps and `words` words of allocation, both
/// integers; a negative limit allows nothing. Returns the budget, for the
/// entry points that end it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_budget_enter(context: *mut Context, steps: Word, words: Word) -> Word {
    unsafe {
        let scheduler = scheduler(context);
        let id = scheduler.budgets.len();
        let limit = |word| u64::try_from(int(word)).unwrap_or(0);
        let parent = scheduler.current().budget.replace(id);
        scheduler.budgets.push(Budget {
            steps: limit(steps),
            words: limit(words),
            parent,
            exhausted: None,
            live: true,
        });
        amb_int_new(id as i64)
    }
}

/// Leaves `budget`, whose body is over. Returns null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_budget_leave(context: *mut Context, budget: Word) -> Word {
    unsafe {
        let scheduler = scheduler(context);
        let mut parent = None;
        if let Some(budget) = scheduler.budgets.get_mut(int(budget) as usize) {
            budget.live = false;
            parent = budget.parent;
        }
        scheduler.current().budget = parent;
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
        let exhausted = scheduler(context)
            .budgets
            .get(int(budget) as usize)
            .and_then(|budget| budget.exhausted);
        let Some(limit) = exhausted else {
            return UNWIND;
        };
        if (*context).fault != Fault::Exhausted as u64 {
            return UNWIND;
        }
        (*context).fault = Fault::None as u64;
        let tag = format!("{FAULT_PACKAGE}\0{limit}");
        amb_tag(tag.as_ptr(), tag.len() as u64)
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
        let mut spent = false;
        while let Some(id) = next {
            let budget = &mut scheduler.budgets[id];
            if budget.steps < steps {
                budget.exhausted.get_or_insert(LIMITS[0]);
            }
            if budget.words < words {
                budget.exhausted.get_or_insert(LIMITS[1]);
            }
            budget.steps = budget.steps.saturating_sub(steps);
            budget.words = budget.words.saturating_sub(words);
            spent |= budget.exhausted.is_some();
            next = budget.parent;
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
    unsafe {
        match scheduler(context).current().budget {
            Some(id) => amb_int_new(id as i64),
            None => NULL,
        }
    }
}

/// Puts the task inside `budget`, as [`amb_budget_current`] gave it, for
/// as long as a handler bound there runs. If that budget is over, the
/// nearest around it that is not stands in. Returns the budget the task
/// was inside, for [`amb_budget_back`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_budget_switch(context: *mut Context, budget: Word) -> Word {
    unsafe {
        let saved = amb_budget_current(context);
        let scheduler = scheduler(context);
        let mut to = (budget != NULL).then(|| int(budget) as usize);
        while let Some(id) = to
            && !scheduler.budgets[id].live
        {
            to = scheduler.budgets[id].parent;
        }
        scheduler.current().budget = to;
        saved
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
        scheduler(context).current().budget = (saved != NULL).then(|| int(saved) as usize);
        if unwinding == 0 { NULL } else { UNWIND }
    }
}

/// Charges for a value about as large as `value`: what an operation that
/// copies it allocates.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_charge_size(context: *mut Context, value: Word) -> Word {
    unsafe {
        let words = if is_object(value) {
            (*header(value)).size.div_ceil(8) + 1
        } else {
            0
        };
        amb_charge(context, 0, words)
    }
}

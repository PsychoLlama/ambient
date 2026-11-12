//! Budgets: a bound on the steps and the allocation of whatever runs
//! inside one. See `spec/budgets.md`.
//!
//! Metered code charges as it goes. A charge counts against every budget
//! the task is inside, and the first to run out unwinds the task to it.
//! Tasks started inside a budget are inside it too.

use super::{interrupted, scheduler};
use crate::boundary::FAULT_PACKAGE;
use crate::number::amb_int_new;
use crate::object::{fault, header, int, is_object};
use crate::text::amb_tag;
use crate::{Context, Fault, NULL, UNWIND, Word};

pub(super) struct Budget {
    /// What is left of each limit.
    steps: u64,
    words: u64,
    /// The budget this one was entered inside.
    parent: Option<usize>,
    /// The limit that ran out, once one has.
    exhausted: Option<&'static str>,
}

/// Enters a budget of `steps` steps and `words` words of allocation, both
/// integers; a negative limit is none at all. Returns the budget, for the
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
        });
        amb_int_new(id as i64)
    }
}

/// Leaves `budget`, whose body is over. Returns null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_budget_leave(context: *mut Context, budget: Word) -> Word {
    unsafe {
        let scheduler = scheduler(context);
        let parent = scheduler
            .budgets
            .get(int(budget) as usize)
            .and_then(|budget| budget.parent);
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
        if interrupted(context) {
            return fault(context, Fault::Cancelled);
        }
        let mut spent = false;
        while let Some(id) = next {
            let budget = &mut scheduler.budgets[id];
            if budget.steps < steps {
                budget.exhausted.get_or_insert("steps");
            }
            if budget.words < words {
                budget.exhausted.get_or_insert("allocation");
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

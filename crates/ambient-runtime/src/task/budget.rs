//! Budgets: a bound on the steps and the allocation of whatever runs
//! inside one. See `spec/budgets.md`.
//!
//! Metered code charges as it goes. A charge counts against every budget
//! the task is inside, and the first to run out unwinds the task to it.
//! Tasks started inside a budget are inside it too.

use alloc::vec::Vec;

use super::scheduler;
use crate::number::amb_int_new;
use crate::object::{KIND_LIST, KIND_RECORD, fault, header, int, is_object, kind};
use crate::record;
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
    /// The tasks that have unwound because it ran out. What one of
    /// them does after that is a cleanup, which draws on [`GRACE`].
    told: Vec<usize>,
    /// Whether its body is still running.
    live: bool,
}

/// What a budget that ran out allows besides, in steps and in words,
/// for the cleanups it unwinds through. It is small and fixed, so a
/// budget still bounds what runs inside it.
const GRACE: u64 = 1_000;

impl Budget {
    /// Takes `steps` and `words` from the budget for `task`, and says
    /// whether the task is to unwind: the budget cannot afford them, or
    /// ran out in another task and this one has yet to hear.
    fn charge(&mut self, task: usize, steps: u64, words: u64) -> bool {
        let short = [self.steps < steps, self.words < words];
        if self.exhausted.is_none() {
            let Some(limit) = short.iter().position(|short| *short) else {
                self.steps -= steps;
                self.words -= words;
                return false;
            };
            self.exhausted = Some(LIMITS[limit]);
            (self.steps, self.words) = (GRACE, GRACE);
        } else if self.told.contains(&task) {
            // A cleanup, drawing on what the budget allows for one.
            self.steps = self.steps.saturating_sub(steps);
            self.words = self.words.saturating_sub(words);
            return short.contains(&true);
        }
        self.told.push(task);
        true
    }
}

impl super::Scheduler {
    /// Takes the task `task`, which is over, out of what the budgets
    /// from `innermost` outward keep of the tasks they told: its number
    /// is another task's from here on.
    pub(super) fn forget(&mut self, task: usize, innermost: Option<usize>) {
        let mut next = innermost;
        while let Some(budget) = next.and_then(|id| self.budgets.get_mut(id)) {
            budget.told.retain(|told| *told != task);
            next = budget.parent;
        }
    }
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
            told: Vec::new(),
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
        let task = scheduler.current;
        let mut spent = false;
        while let Some(id) = next {
            let budget = &mut scheduler.budgets[id];
            spent |= budget.charge(task, steps, words);
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

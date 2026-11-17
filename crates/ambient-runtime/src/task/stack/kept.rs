//! Stacks as a platform keeps them: what the runtime holds of each
//! task's, and the turns it gives them.

use super::super::depth::NOWHERE;
use super::super::{ROOT, Scheduler, run, scheduler};
use crate::object::{amb_release, amb_retain};
use crate::{Context, UNWIND, Word};

/// What a platform that keeps a task as a stack gives the runtime.
#[repr(C)]
pub struct Stacks {
    /// Runs `body` with the context and `argument` on a stack of its
    /// own, until it parks or returns. Gives the stack if it parked,
    /// and zero if it returned.
    pub start: unsafe extern "C" fn(*mut Context, Body, u64) -> u64,
    /// Goes on with a parked stack until it parks again or its body
    /// returns. Gives the stack if it parked, and zero if it returned:
    /// the stack is then gone.
    pub resume: unsafe extern "C" fn(*mut Context, u64) -> u64,
    /// Parks the stack that is running. Returns once it is resumed.
    pub park: unsafe extern "C" fn(*mut Context),
    /// Gives up a parked stack that will not be resumed. Nothing on it
    /// runs again, so what it held is lost.
    pub drop: unsafe extern "C" fn(*mut Context, u64),
}

/// What runs on a stack.
pub type Body = unsafe extern "C" fn(*mut Context, u64);

/// What a task keeps where it is a stack.
#[derive(Default)]
pub(in super::super) struct Kept {
    /// Its stack while it is parked. Zero if it has none.
    stack: u64,
    /// How deep whatever started its stack was.
    from: Word,
    /// What its stack's body gave, until its turn takes it.
    outcome: Word,
    /// For the root: what the host called it with.
    handed: Option<Word>,
    /// How deep the native code it is beneath stands.
    pinned: Word,
}

impl Kept {
    /// Says how deep the native code the task is beneath stands: where
    /// a task given its turn there counts from. Gives what was said
    /// before.
    pub(in super::super) fn stands(&mut self, depth: Word) -> Word {
        core::mem::replace(&mut self.pinned, depth)
    }

    /// How deep the native code the task is beneath stands.
    pub(in super::super) fn stood(&self) -> Word {
        self.pinned
    }

    /// What the host called the root with, if this is the root's.
    pub(in super::super) fn handed(&self) -> Option<Word> {
        self.handed
    }
}

/// The stacks of the platform this context runs on, if it keeps tasks
/// so.
pub(in super::super) unsafe fn of<'a>(context: *mut Context) -> Option<&'a Stacks> {
    unsafe { ((*context).stacks as usize as *const Stacks).as_ref() }
}

/// Parks the task that is running.
pub(super) unsafe fn park(context: *mut Context, stacks: &Stacks) {
    unsafe { (stacks.park)(context) }
}

/// Gives the task `id` its turn on its own stack, which is made if it
/// has none. What it calls first counts from `depth`. Gives what its
/// body gave, or [`UNWIND`] with the mode suspending if it parked.
pub(in super::super) unsafe fn turn(
    context: *mut Context,
    stacks: &Stacks,
    id: usize,
    depth: Word,
) -> Word {
    unsafe {
        let kept = &mut scheduler(context).task(id).kept;
        let parked = if kept.stack == 0 {
            kept.from = depth;
            (stacks.start)(context, body, id as u64)
        } else {
            (stacks.resume)(context, kept.stack)
        };
        let kept = &mut scheduler(context).task(id).kept;
        kept.stack = parked;
        if parked != 0 {
            return UNWIND;
        }
        core::mem::take(&mut kept.outcome)
    }
}

/// What a task's stack runs: its thunk, and then nothing.
unsafe extern "C" fn body(context: *mut Context, id: u64) {
    unsafe {
        let id = id as usize;
        let depth = scheduler(context).task(id).kept.from;
        let result = run(context, id, depth);
        scheduler(context).task(id).kept.outcome = result;
    }
}

/// What a host's call of `callee` is where the root is a stack: the
/// call is made on one, and made again it goes on with that stack.
pub(in super::super) unsafe fn root(
    context: *mut Context,
    stacks: &Stacks,
    callee: Word,
    args: Word,
) -> Word {
    unsafe {
        let task = scheduler(context).task(ROOT);
        if task.kept.stack == 0 {
            amb_retain(callee);
            amb_retain(args);
            task.thunk = callee;
            task.kept.handed = Some(args);
        }
        let result = turn(context, stacks, ROOT, NOWHERE);
        let task = scheduler(context).task(ROOT);
        if task.kept.stack == 0 {
            amb_release(core::mem::take(&mut task.thunk));
            if let Some(args) = task.kept.handed.take() {
                amb_release(args);
            }
        }
        result
    }
}

impl Scheduler {
    /// Gives up the stack of every task that still has one: the
    /// context is going, and they with it.
    pub(crate) unsafe fn abandon(&mut self, context: *mut Context) {
        let Some(stacks) = (unsafe { of(context) }) else {
            return;
        };
        for task in self.tasks.iter_mut().flatten() {
            let stack = core::mem::take(&mut task.kept.stack);
            if stack != 0 {
                unsafe { (stacks.drop)(context, stack) };
            }
        }
    }
}

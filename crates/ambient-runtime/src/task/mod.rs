//! Tasks: suspension, a scheduler, and joining. See `spec/tasks.md` and
//! `spec/backends.md`.
//!
//! A task suspends by unwinding, unless its platform keeps it as a
//! stack: see [`stack`]. Something in the runtime sets the
//! context's mode to [`SUSPENDING`] and returns [`UNWIND`]; each compiled
//! function on the way out saves its locals in a frame and returns too.
//! To resume, the task's body is called again with the mode
//! [`RESUMING`]: each function takes its frame back, goes to where it was,
//! and calls what it had called, until the call that suspended is made
//! again. That call finds the mode still `RESUMING`, which is how it
//! knows it is arriving rather than starting.
//!
//! Scheduling is first come, first served, on one thread, so a run is
//! reproducible.

use alloc::vec;
use alloc::vec::Vec;

mod back;
mod budget;
mod depth;
mod frame;
mod group;
mod join;
mod places;
mod stack;
mod tail;
mod wait;

use alloc::collections::VecDeque;

pub use back::{amb_call_among, amb_call_back};
pub use budget::{
    LIMITS, amb_budget_back, amb_budget_caught, amb_budget_current, amb_budget_enter,
    amb_budget_leave, amb_budget_switch, amb_charge, amb_charge_read, amb_charge_size,
    amb_charge_tasks,
};
pub use depth::{DEPTH, FRAME, NATIVE, amb_depth, amb_enter};
pub use frame::{amb_frame_load, amb_frame_point, amb_frame_push, amb_frame_save};
pub use group::{amb_group_close, amb_group_end, amb_group_new, amb_spawn};
pub use join::{amb_all, amb_first};
pub(crate) use stack::waiting;
pub use stack::{Body, Stacks};
use tail::start;
pub use tail::{amb_bounce, amb_start, amb_tail, amb_tail_arg};
pub use wait::{FORSAKEN, amb_answer, amb_await, amb_native_wait, amb_refuse, amb_strand};

use crate::object::amb_release;
use crate::{Context, Fault, UNWIND, Word};

/// The modes of a context. Compiled code reads the mode after a call
/// that unwound, and on entry.
pub const RUNNING: u64 = 0;
pub const SUSPENDING: u64 = 1;
pub const RESUMING: u64 = 2;
/// What [`amb_schedule`] leaves when no task can run until the host
/// answers one that waits on it.
pub const WAITING: u64 = 3;

/// The task a program's entry runs as.
const ROOT: usize = 0;

/// What one function held when its task suspended.
struct Frame {
    /// Which of its calls it was in.
    point: u64,
    /// For a call [`tail::amb_bounce`] was making, its code.
    position: u64,
    locals: Vec<Word>,
}

#[derive(Default)]
struct Task {
    /// The frames of a suspended task, innermost first.
    frames: Vec<Frame>,
    /// What to call to run it: a closure that takes nothing. Zero for
    /// the root, which the host calls.
    thunk: Word,
    started: bool,
    cancelled: bool,
    /// How many cleanups it is inside. Cancellation waits for them.
    masked: usize,
    /// How many groups it is in the body of have a task that failed. It
    /// unwinds to the outermost of them as if cancelled.
    disturbed: usize,
    /// Whether it is waiting its turn.
    queued: bool,
    /// The join it was started by, and its place there.
    place: Option<(usize, usize)>,
    /// The join it started and is waiting on.
    join: Option<usize>,
    /// The cell it is waiting on.
    cell: Option<Word>,
    /// The innermost budget it runs inside.
    budget: Option<usize>,
    /// Its wait on the host, if native code said it would answer later.
    wait: wait::Wait,
    /// Where it is with native code it called.
    beneath: Beneath,
    /// What it keeps where a task is a stack, and nowhere else: see
    /// [`stack`].
    kept: stack::Kept,
}

/// Where a task is with native code that called back into it.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Beneath {
    /// No native code it called is on the stack.
    #[default]
    Free,
    /// Native code it called is on the stack beneath it, so only the
    /// call back that native code made can resume it.
    Pinned,
    /// It is to unwind to that native code, since nothing that can run
    /// while the native code stands will let it go on.
    Stuck,
}

/// What the starter of tasks gets of them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Gives {
    /// Nothing: they are a group's, whose starter runs on, and what
    /// they give is dropped.
    Nothing,
    /// What each gave, as a tuple.
    Tuple,
    /// What each gave, as a list.
    List,
    /// What the first to be over gave; it cancels the rest.
    First,
}

/// Tasks started together, and what has come of them.
struct Join {
    parent: usize,
    children: Vec<usize>,
    results: Vec<Word>,
    remaining: usize,
    failure: Option<Failure>,
    gives: Gives,
    /// The place of the first to be over with a result.
    winner: Option<usize>,
    /// Whether its tasks have been told to stop.
    closing: bool,
}

/// How a task ended, if not with a value: what the context held.
struct Failure {
    fault: u64,
    token: u64,
    args: u64,
    /// The trace of a fault, or null.
    trace: Word,
}

impl Failure {
    /// Gives up what the failure holds: it is not going anywhere.
    unsafe fn drop(self) {
        unsafe {
            amb_release(self.args);
            amb_release(self.trace);
        }
    }
}

/// In a join's list of the tasks it started, one that is over: a
/// task's number is another's once it is.
const NOBODY: usize = usize::MAX;

pub(crate) struct Scheduler {
    /// Every task, by its number: the place it has here until it is
    /// over, which the next task started then takes.
    tasks: Vec<Option<Task>>,
    /// The places in `tasks` no task has.
    idle: Vec<usize>,
    /// Every join, numbered as tasks are.
    joins: Vec<Option<Join>>,
    /// The places in `joins` no join has.
    closed: Vec<usize>,
    ready: VecDeque<usize>,
    current: usize,
    /// Every budget entered, by the number compiled code knows it by.
    budgets: Vec<budget::Budget>,
    /// Whether every task was once found waiting on something that would
    /// never happen. From then on nothing holds cancellation off.
    stalled: bool,
    waits: wait::Waits,
}

impl Scheduler {
    pub(crate) fn new() -> Self {
        Self {
            tasks: vec![Some(Task::default())],
            idle: Vec::new(),
            joins: Vec::new(),
            closed: Vec::new(),
            ready: VecDeque::new(),
            current: ROOT,
            budgets: Vec::new(),
            stalled: false,
            waits: wait::Waits::default(),
        }
    }

    fn task(&mut self, id: usize) -> &mut Task {
        self.tasks[id].get_or_insert_with(Task::default)
    }

    fn current(&mut self) -> &mut Task {
        self.task(self.current)
    }

    fn queue(&mut self, id: usize) {
        let task = self.task(id);
        if !task.queued {
            task.queued = true;
            self.ready.push_back(id);
        }
    }

    /// Makes a task unwind from wherever it is waiting, or never start.
    fn cancel(&mut self, id: usize) {
        let task = self.task(id);
        task.cancelled = true;
        task.cell = None;
        self.queue(id);
    }

    /// Whether the current task is to unwind where it is: it has been
    /// cancelled, and is not inside a cleanup.
    fn interrupted(&mut self) -> bool {
        let stalled = self.stalled;
        let task = self.current();
        // One that is stuck beneath native code would wait for ever in
        // a cleanup too.
        task.beneath == Beneath::Stuck
            || (task.cancelled || task.disturbed > 0) && (task.masked == 0 || stalled)
    }

    /// Notes that the current task has entered a cleanup, or left one.
    fn mask(&mut self, entering: bool) {
        let task = self.current();
        task.masked = if entering {
            task.masked + 1
        } else {
            task.masked.saturating_sub(1)
        };
    }

    /// Makes `id` the current task, to be resumed by whatever calls
    /// what it suspended in again: the host, for the root, or the call
    /// back it suspended beneath.
    unsafe fn resumes(&mut self, context: *mut Context, id: usize) {
        self.current = id;
        self.task(id).queued = false;
        unsafe { (*context).mode = RESUMING };
    }
}

/// The scheduler of a context.
///
/// This is the only way to it, and it is exclusive: whoever holds what
/// this returns must be done with it before anything else asks, which
/// means before calling compiled code, which comes back into the
/// runtime, and before calling any function here that takes a context.
unsafe fn scheduler<'a>(context: *mut Context) -> &'a mut Scheduler {
    unsafe { &mut *((*context).scheduler as usize as *mut Scheduler) }
}

/// Whether the call being made is one a resuming task had suspended in.
/// If so the task is running again from here.
pub(crate) unsafe fn arriving(context: *mut Context) -> bool {
    unsafe {
        let resuming = (*context).mode == RESUMING;
        (*context).mode = RUNNING;
        resuming
    }
}

/// Suspends the current task. Always returns [`UNWIND`].
///
/// Everything that suspends comes through here, by name and never
/// inlined: a program that does not reach this cannot suspend, and is
/// linked without what gives suspended tasks their turns
/// (`spec/linking.md`).
#[unsafe(no_mangle)]
#[inline(never)]
pub unsafe extern "C" fn amb_suspend(context: *mut Context) -> Word {
    unsafe {
        (*context).mode = SUSPENDING;
    }
    UNWIND
}

/// Whether the current task is to unwind where it is: it has been
/// cancelled, and is not inside a cleanup.
pub(crate) unsafe fn interrupted(context: *mut Context) -> bool {
    unsafe { scheduler(context) }.interrupted()
}

/// Notes that the current task has entered a cleanup, or left one.
pub(crate) unsafe fn mask(context: *mut Context, entering: bool) {
    unsafe { scheduler(context) }.mask(entering);
}

/// Makes the current task wait on `cell`, or stop waiting.
pub(crate) unsafe fn wait_on(context: *mut Context, cell: Option<Word>) {
    unsafe { scheduler(context) }.current().cell = cell;
}

/// Gives every task waiting on `cell` its turn.
pub(crate) unsafe fn wake(context: *mut Context, cell: Word) {
    let scheduler = unsafe { scheduler(context) };
    for id in 0..scheduler.tasks.len() {
        let waiting = scheduler.tasks[id]
            .as_ref()
            .is_some_and(|task| task.cell == Some(cell));
        if waiting {
            scheduler.task(id).cell = None;
            scheduler.queue(id);
        }
    }
}

/// Runs tasks until the root can go on, then makes it the current task
/// and sets the mode so that calling the entry again resumes it.
///
/// If nothing can run and a task waits on the host, this returns with
/// the mode [`WAITING`]: the host hands an answer over and calls this
/// again.
///
/// If nothing can run and nothing is coming, every task is waiting for
/// something that will not happen. The root is then cancelled, so that
/// calling the entry again unwinds it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_schedule(context: *mut Context) {
    unsafe {
        loop {
            let scheduler = scheduler(context);
            if scheduler.awaits() {
                return wait::idle(context);
            }
            let id = scheduler.ready.pop_front().unwrap_or_else(|| {
                scheduler.stalled = true;
                scheduler.task(ROOT).cancelled = true;
                ROOT
            });
            if id == ROOT {
                return scheduler.resumes(context, id);
            }
            turn(context, id, depth::NOWHERE);
        }
    }
}

/// Gives the task `id`, which is not the root, its turn: starts it or
/// resumes it, from `depth`, and notes what came of it.
unsafe fn turn(context: *mut Context, id: usize, depth: Word) {
    unsafe {
        let scheduler = scheduler(context);
        // A turn is only ever waiting for a task that is not over.
        if !scheduler.live(id) {
            return;
        }
        scheduler.current = id;
        let task = scheduler.task(id);
        task.queued = false;
        if task.cancelled && !task.started {
            scheduler.finish(id, None);
            return;
        }
        (*context).mode = if task.started { RESUMING } else { RUNNING };
        task.started = true;
        let result = match stack::of(context) {
            Some(stacks) => stack::turn(context, stacks, id, depth),
            None => run(context, id, depth),
        };
        ended(context, id, result);
    }
}

/// Calls what the task `id` runs, from `depth`: its thunk, or for the
/// root what the host called.
unsafe fn run(context: *mut Context, id: usize, depth: Word) -> Word {
    unsafe {
        let task = scheduler(context).task(id);
        let thunk = task.thunk;
        // The root is run from here only where a task is a stack.
        if let Some((args, metered)) = task.kept.handed() {
            return start(context, thunk, args, metered, depth);
        }
        // A task inside a budget is metered, as what started it was.
        let metered = u64::from(task.budget.is_some());
        // A thunk takes nothing but the bindings every function is
        // handed, and it is handed none: it captured its own.
        let nothing = crate::sequence::amb_sequence_new(0, 1);
        let result = start(context, thunk, nothing, metered, depth);
        amb_release(nothing);
        result
    }
}

/// What a task's body returning means for the task.
unsafe fn ended(context: *mut Context, id: usize, result: Word) {
    unsafe {
        let scheduler = scheduler(context);
        if result != UNWIND {
            scheduler.finish(id, Some(result));
            return;
        }
        if (*context).mode == SUSPENDING {
            (*context).mode = RUNNING;
            return;
        }
        // It unwound. Unless that was its own cancellation, it takes the
        // tasks started with it down, and their starter after them.
        let failure = Failure {
            fault: (*context).fault,
            token: core::mem::take(&mut (*context).abort_token),
            args: core::mem::take(&mut (*context).abort_args),
            trace: crate::trace::detach(context),
        };
        (*context).fault = Fault::None as u64;
        let cancelled = scheduler.task(id).cancelled;
        let place = scheduler.task(id).place;
        let join = place.and_then(|(join, _)| scheduler.joins[join].as_mut());
        match join {
            Some(join) if !cancelled && join.failure.is_none() => {
                join.failure = Some(failure);
                let disturbs = (join.gives == Gives::Nothing).then_some(join.parent);
                for sibling in join.children.clone() {
                    if sibling != id && scheduler.live(sibling) {
                        scheduler.cancel(sibling);
                    }
                }
                // The starter of a group is in its body, and leaves it.
                if let Some(parent) = disturbs {
                    scheduler.task(parent).disturbed += 1;
                    scheduler.queue(parent);
                }
            }
            _ => failure.drop(),
        }
        scheduler.finish(id, None);
    }
}

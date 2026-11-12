//! Tasks: suspension, a scheduler, and joining. See `spec/tasks.md` and
//! `spec/backends.md`.
//!
//! A task suspends by unwinding. Something in the runtime sets the
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

use std::collections::VecDeque;

use crate::object::{amb_release, amb_retain, fault, words};
use crate::sequence::{amb_sequence_new, amb_sequence_set};
use crate::{Context, Fault, NULL, UNWIND, Word};

/// The modes of a context. Compiled code reads the mode after a call
/// that unwound, and on entry.
pub const RUNNING: u64 = 0;
pub const SUSPENDING: u64 = 1;
pub const RESUMING: u64 = 2;

/// The task a program's entry runs as.
const ROOT: usize = 0;

/// What one function held when its task suspended.
struct Frame {
    /// Which of its calls it was in.
    point: u64,
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
    /// Whether it is waiting its turn.
    queued: bool,
    /// The join it was started by, and its place there.
    place: Option<(usize, usize)>,
    /// The join it started and is waiting on.
    join: Option<usize>,
    /// The cell it is waiting on.
    cell: Option<Word>,
}

/// Tasks started together, and what has come of them.
struct Join {
    parent: usize,
    children: Vec<usize>,
    results: Vec<Word>,
    remaining: usize,
    failure: Option<Failure>,
    /// Whether the first to be over cancels the rest.
    first: bool,
    /// The place of the first to be over with a result.
    winner: Option<usize>,
}

/// How a task ended, if not with a value: what the context held.
struct Failure {
    fault: u64,
    token: u64,
    args: u64,
}

pub(crate) struct Scheduler {
    tasks: Vec<Option<Task>>,
    joins: Vec<Option<Join>>,
    ready: VecDeque<usize>,
    current: usize,
    /// Whether every task was once found waiting on something that would
    /// never happen. From then on nothing holds cancellation off.
    stalled: bool,
}

impl Scheduler {
    pub(crate) fn new() -> Self {
        Self {
            tasks: vec![Some(Task::default())],
            joins: Vec::new(),
            ready: VecDeque::new(),
            current: ROOT,
            stalled: false,
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

    /// Records that a task is over, with `result` if it gave one.
    unsafe fn finish(&mut self, id: usize, result: Option<Word>) {
        let Some(task) = self.tasks[id].take() else {
            return;
        };
        unsafe { amb_release(task.thunk) };
        let Some((join, place)) = task.place else {
            return;
        };
        let Some(join) = self.joins[join].as_mut() else {
            return;
        };
        join.results[place] = result.unwrap_or(NULL);
        join.remaining -= 1;
        let parent = join.parent;
        let remaining = join.remaining;
        let won = join.first && result.is_some() && join.winner.is_none() && join.failure.is_none();
        let losers = if won {
            join.winner = Some(place);
            join.children.clone()
        } else {
            Vec::new()
        };
        for loser in losers {
            if self.tasks[loser].is_some() {
                self.cancel(loser);
            }
        }
        if remaining == 0 {
            self.queue(parent);
        }
    }
}

/// The scheduler of a context. Nothing holds this across a call into
/// compiled code, which may come back into the runtime.
unsafe fn scheduler<'a>(context: *mut Context) -> &'a mut Scheduler {
    unsafe { &mut *((*context).scheduler as usize as *mut Scheduler) }
}

/// Starts a frame for the function that is suspending, at `point`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_frame_push(context: *mut Context, point: u64) {
    let locals = Vec::new();
    unsafe { scheduler(context) }
        .current()
        .frames
        .push(Frame { point, locals });
}

/// Adds a local to the frame being made. The frame takes the local's
/// reference; the function gives it up by returning without releasing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_frame_save(context: *mut Context, value: Word) {
    if let Some(frame) = unsafe { scheduler(context) }.current().frames.last_mut() {
        frame.locals.push(value);
    }
}

/// The point the function being resumed was at.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_frame_point(context: *mut Context) -> u64 {
    let frames = &unsafe { scheduler(context) }.current().frames;
    frames.last().map_or(0, |frame| frame.point)
}

/// A local of the function being resumed, which takes it back.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_frame_load(context: *mut Context, index: u64) -> Word {
    let frames = &unsafe { scheduler(context) }.current().frames;
    frames
        .last()
        .and_then(|frame| frame.locals.get(index as usize).copied())
        .unwrap_or(0)
}

/// Ends the frame of the function being resumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_frame_drop(context: *mut Context) {
    unsafe { scheduler(context) }.current().frames.pop();
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
pub(crate) unsafe fn suspend(context: *mut Context) -> Word {
    unsafe {
        (*context).mode = SUSPENDING;
    }
    UNWIND
}

/// Whether the current task is to unwind where it is: it has been
/// cancelled, and is not inside a cleanup.
pub(crate) unsafe fn interrupted(context: *mut Context) -> bool {
    let scheduler = unsafe { scheduler(context) };
    let stalled = scheduler.stalled;
    let task = scheduler.current();
    task.cancelled && (task.masked == 0 || stalled)
}

/// Notes that the current task has entered a cleanup, or left one.
pub(crate) unsafe fn mask(context: *mut Context, entering: bool) {
    let task = unsafe { scheduler(context) }.current();
    task.masked = if entering {
        task.masked + 1
    } else {
        task.masked.saturating_sub(1)
    };
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

/// Runs each of `thunks`, a tuple of closures that take nothing, as a
/// task, and gives the tuple of their results once every one is over.
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
    unsafe {
        if arriving(context) {
            return joined(context);
        }
        if interrupted(context) {
            return fault(context, Fault::Cancelled);
        }
        let scheduler = scheduler(context);
        let parent = scheduler.current;
        let join = scheduler.joins.len();
        let thunks = words(thunks);
        let mut children = Vec::new();
        for (place, thunk) in thunks.iter().enumerate() {
            amb_retain(*thunk);
            let id = scheduler.tasks.len();
            scheduler.tasks.push(Some(Task {
                thunk: *thunk,
                place: Some((join, place)),
                ..Task::default()
            }));
            scheduler.queue(id);
            children.push(id);
        }
        scheduler.joins.push(Some(Join {
            parent,
            children,
            results: vec![NULL; thunks.len()],
            remaining: thunks.len(),
            failure: None,
            first,
            winner: None,
        }));
        scheduler.current().join = Some(join);
        if thunks.is_empty() && !first {
            scheduler.queue(parent);
        }
        suspend(context)
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
        if unfinished && !interrupted(context) {
            // It was cancelled inside a cleanup, which goes on waiting.
            return suspend(context);
        }
        if unfinished {
            // Only cancellation wakes a task whose tasks are not over. It
            // cannot leave before they have, so it passes it on and waits.
            let children = scheduler.joins[id]
                .as_ref()
                .map(|join| join.children.clone())
                .unwrap_or_default();
            for child in children {
                if scheduler.tasks[child].is_some() {
                    scheduler.cancel(child);
                }
            }
            return suspend(context);
        }
        scheduler.current().join = None;
        let Some(join) = scheduler.joins[id].take() else {
            return fault(context, Fault::Cancelled);
        };
        let failure = match join.failure {
            _ if interrupted(context) => Some(Fault::Cancelled as u64),
            Some(failure) => {
                (*context).abort_token = failure.token;
                (*context).abort_args = failure.args;
                Some(failure.fault)
            }
            None => None,
        };
        if let Some(code) = failure {
            join.results.iter().for_each(|result| amb_release(*result));
            (*context).fault = code;
            return UNWIND;
        }
        if join.first {
            let winner = join.winner.unwrap_or_default();
            for (place, result) in join.results.iter().enumerate() {
                if place != winner {
                    amb_release(*result);
                }
            }
            return join.results.get(winner).copied().unwrap_or(NULL);
        }
        let tuple = amb_sequence_new(0, join.results.len() as u64);
        for (index, result) in join.results.iter().enumerate() {
            amb_sequence_set(tuple, index as u64, *result);
            amb_release(*result);
        }
        tuple
    }
}

type Thunk = unsafe extern "C" fn(*mut Context, Word, Word) -> Word;

/// Runs tasks until the root can go on, then makes it the current task
/// and sets the mode so that calling the entry again resumes it.
///
/// If nothing can run and the root still cannot go on, every task is
/// waiting for something that will not happen. The root is then
/// cancelled, so that calling the entry again unwinds it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_schedule(context: *mut Context) {
    unsafe {
        loop {
            let scheduler = scheduler(context);
            let id = scheduler.ready.pop_front().unwrap_or_else(|| {
                scheduler.stalled = true;
                scheduler.task(ROOT).cancelled = true;
                ROOT
            });
            scheduler.current = id;
            let task = scheduler.task(id);
            task.queued = false;
            if id == ROOT {
                (*context).mode = RESUMING;
                return;
            }
            if task.cancelled && !task.started {
                scheduler.finish(id, None);
                continue;
            }
            (*context).mode = if task.started { RESUMING } else { RUNNING };
            task.started = true;
            let thunk = task.thunk;
            let definition = words(thunk)[0] as usize as *const u64;
            let code = std::mem::transmute::<usize, Thunk>(definition.read() as usize);
            let result = code(context, thunk, NULL);
            ended(context, id, result);
        }
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
            token: std::mem::take(&mut (*context).abort_token),
            args: std::mem::take(&mut (*context).abort_args),
        };
        (*context).fault = Fault::None as u64;
        let cancelled = scheduler.task(id).cancelled;
        let place = scheduler.task(id).place;
        let join = place.and_then(|(join, _)| scheduler.joins[join].as_mut());
        match join {
            Some(join) if !cancelled && join.failure.is_none() => {
                join.failure = Some(failure);
                for sibling in join.children.clone() {
                    if sibling != id && scheduler.tasks[sibling].is_some() {
                        scheduler.cancel(sibling);
                    }
                }
            }
            _ => amb_release(failure.args),
        }
        scheduler.finish(id, None);
    }
}

//! A task as a stack of its own: what the runtime asks of a platform
//! that can keep one (`ambient_runtime::Stacks`). See "Suspension" in
//! `docs/spec/backends.md`.
//!
//! Each stack has the room a program may take ([`ROOM`]), reserved and
//! not touched until used. One that is done with is kept for the next
//! task, so starting a task maps nothing once a few have run.

use std::cell::{Cell, RefCell};

use ambient_runtime::{Context, Stacks};
use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, CoroutineResult, Yielder};

use crate::stack::ROOM;

/// What the runtime is given for a native program.
pub(super) static STACKS: Stacks = Stacks {
    start,
    resume,
    park,
    drop: dropped,
};

/// How many stacks that are done with are kept for the tasks to come.
const KEPT: usize = 64;

/// A task's stack, and what is running on it.
struct Fiber {
    coroutine: Coroutine<(), (), (), DefaultStack>,
    /// What parks it, which only what runs on it is handed.
    yielder: *const Cell<*const Yielder<(), ()>>,
}

thread_local! {
    /// The stacks that are running, innermost last: one resumed from
    /// another is above it.
    static RUNNING: RefCell<Vec<*const Yielder<(), ()>>> = const { RefCell::new(Vec::new()) };
    /// Stacks that are done with.
    static SPARE: RefCell<Vec<DefaultStack>> = const { RefCell::new(Vec::new()) };
}

/// The fiber the runtime knows as `stack`: its address.
fn fiber(stack: u64) -> *mut Fiber {
    usize::try_from(stack).unwrap_or(0) as *mut Fiber
}

/// Goes on with `fiber` until it parks or is over. Gives it back if it
/// parked, and zero if it is over.
fn run(fiber: Box<Fiber>) -> u64 {
    let mut fiber = fiber;
    let outcome = fiber.coroutine.resume(());
    RUNNING.with_borrow_mut(Vec::pop);
    match outcome {
        CoroutineResult::Yield(()) => Box::into_raw(fiber) as u64,
        CoroutineResult::Return(()) => {
            retire(*fiber);
            0
        }
    }
}

/// Keeps the stack of a fiber that is over, and frees what it shared
/// with its body.
fn retire(fiber: Fiber) {
    // SAFETY: the cell was made in `start` and only the body, which is
    // over, read it.
    drop(unsafe { Box::from_raw(fiber.yielder.cast_mut()) });
    let stack = fiber.coroutine.into_stack();
    SPARE.with_borrow_mut(|spare| {
        if spare.len() < KEPT {
            spare.push(stack);
        }
    });
}

unsafe extern "C" fn start(
    context: *mut Context,
    body: ambient_runtime::Body,
    argument: u64,
) -> u64 {
    let stack = SPARE
        .with_borrow_mut(Vec::pop)
        .or_else(|| DefaultStack::new(ROOM).ok());
    // A stack that cannot be had is a host out of memory, as one that
    // cannot grow is.
    let Some(stack) = stack else {
        std::process::abort()
    };
    let yielder = Box::into_raw(Box::new(Cell::new(std::ptr::null())));
    let said = yielder.cast_const();
    let coroutine = Coroutine::with_stack(stack, move |yielder: &Yielder<(), ()>, ()| {
        // SAFETY: the cell outlives the body: `retire` frees it.
        unsafe { (*said).set(std::ptr::from_ref(yielder)) };
        RUNNING.with_borrow_mut(|running| running.push(std::ptr::from_ref(yielder)));
        // SAFETY: the runtime handed both over to be called so.
        unsafe { body(context, argument) };
    });
    run(Box::new(Fiber {
        coroutine,
        yielder: said,
    }))
}

unsafe extern "C" fn resume(_: *mut Context, stack: u64) -> u64 {
    // SAFETY: the runtime hands back only what `run` gave it, once.
    let fiber = unsafe { Box::from_raw(fiber(stack)) };
    // SAFETY: a fiber that parked has run, and so has said.
    let yielder = unsafe { (*fiber.yielder).get() };
    RUNNING.with_borrow_mut(|running| running.push(yielder));
    run(fiber)
}

unsafe extern "C" fn park(_: *mut Context) {
    let yielder = RUNNING.with_borrow(|running| running.last().copied());
    let Some(yielder) = yielder else {
        // The runtime parks only what it runs on a stack.
        std::process::abort()
    };
    // SAFETY: the innermost running stack is the one this is called
    // on, and its yielder lives as long as its body.
    unsafe { (*yielder).suspend(()) };
}

unsafe extern "C" fn dropped(_: *mut Context, stack: u64) {
    // SAFETY: the runtime hands back only what `run` gave it, once.
    let mut fiber = unsafe { Box::from_raw(fiber(stack)) };
    // SAFETY: nothing on the stack runs again, and what is on it is
    // compiled code and the runtime, which keep nothing a destructor
    // would release.
    unsafe { fiber.coroutine.force_reset() };
    retire(*fiber);
}

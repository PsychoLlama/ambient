//! Stacks where no platform keeps one: WebAssembly, whose code cannot
//! leave its stack and come back. Nothing here can be made, so
//! whatever asks for a task's stack compiles to nothing.

use super::super::Scheduler;
use crate::{Context, Word};

/// What a platform that keeps a task as a stack gives the runtime.
/// None does here.
pub enum Stacks {}

/// What runs on a stack.
pub type Body = unsafe extern "C" fn(*mut Context, u64);

/// What a task keeps where it is a stack: here, nothing.
#[derive(Default)]
pub(in super::super) struct Kept;

// Each takes what it takes where a stack is kept, so one caller reads
// either.
#[allow(clippy::unused_self)]
impl Kept {
    pub(in super::super) fn stands(&mut self, depth: Word) -> Word {
        depth
    }

    pub(in super::super) fn stood(&self) -> Word {
        0
    }

    pub(in super::super) fn handed(&self) -> Option<Word> {
        None
    }
}

pub(in super::super) unsafe fn of<'a>(_: *mut Context) -> Option<&'a Stacks> {
    None
}

pub(super) unsafe fn park(_: *mut Context, stacks: &Stacks) {
    match *stacks {}
}

pub(in super::super) unsafe fn turn(_: *mut Context, stacks: &Stacks, _: usize, _: Word) -> Word {
    match *stacks {}
}

pub(in super::super) unsafe fn root(_: *mut Context, stacks: &Stacks, _: Word, _: Word) -> Word {
    match *stacks {}
}

#[allow(clippy::unused_self)]
impl Scheduler {
    pub(crate) unsafe fn abandon(&mut self, _: *mut Context) {}
}

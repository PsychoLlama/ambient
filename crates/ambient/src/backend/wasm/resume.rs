//! The code that lets a task suspend in a function and resume in it. See
//! "Suspension" in `docs/spec/backends.md`.
//!
//! A function that may suspend keeps one more local, where it is
//! ([`Emitter::at`]): the last point it reached, and less than any
//! before it has reached one. Points are numbered in the order they are
//! reached, so that one number says everything a function going back to
//! a point must know: a value whose points all come before it was
//! computed already, and anything else is still to do. Each `Let` asks
//! with one comparison, and a point only says its number.
//!
//! What a function pays for suspending is then that number at each
//! point, a test of each value it binds, and one block at each end: one
//! that takes its locals back, one that keeps them.

use ambient_runtime::{Context, Entry, RESUMING, SUSPENDING};
use wasm_encoder::{BlockType, Instruction};

use super::emit::{CONTEXT, Emitter, memory};
use crate::lir::{Expr, Op};

/// Where a function that has reached no point is.
pub(super) const NOWHERE: i64 = -1;

impl Emitter {
    /// Pushes the context's mode.
    fn mode(&mut self) {
        self.emit(&Instruction::LocalGet(CONTEXT));
        self.emit(&Instruction::I64Load(memory(Context::MODE)));
    }

    /// What a function that may be resumed does first: if its task is
    /// resuming, it takes its locals back, last kept first, and then
    /// where it was.
    pub(super) fn restore(&mut self) {
        self.mode();
        self.constant(RESUMING);
        self.emit(&Instruction::I64Eq);
        self.emit(&Instruction::If(BlockType::Empty));
        for local in self.saved().rev() {
            self.emit(&Instruction::LocalGet(CONTEXT));
            self.runtime(Entry::FrameLoad);
            self.set_local(local);
        }
        self.emit(&Instruction::LocalGet(CONTEXT));
        self.runtime(Entry::FramePoint);
        self.emit(&Instruction::LocalSet(self.at));
        self.emit(&Instruction::Else);
        self.emit(&Instruction::I64Const(NOWHERE));
        self.emit(&Instruction::LocalSet(self.at));
        self.emit(&Instruction::End);
    }

    /// Pushes whether the function is before the point `point`: it is
    /// not going back to that point or to one after it.
    pub(super) fn before(&mut self, point: usize) {
        self.emit(&Instruction::LocalGet(self.at));
        self.constant(point as u64);
        self.emit(&Instruction::I64LtS);
    }

    /// Says the function is at no point: a loop's next turn reaches
    /// again the points this one passed.
    pub(super) fn nowhere(&mut self) {
        if self.resumes {
            self.emit(&Instruction::I64Const(NOWHERE));
            self.emit(&Instruction::LocalSet(self.at));
        }
    }

    /// A value a `Let` binds to `local`, put there: unless the function
    /// is going back to a point past all of `points`, in which case the
    /// local has it already.
    pub(super) fn resumed(&mut self, points: (usize, usize), local: usize, value: &Expr) {
        self.before(points.1);
        self.emit(&Instruction::If(BlockType::Empty));
        self.frames.push(false);
        self.bound(local, value);
        self.frames.pop();
        self.emit(&Instruction::End);
    }

    /// An operation the task may suspend in. Leaves one word on the
    /// stack.
    pub(super) fn point(&mut self, point: usize, op: &Op) {
        // Whatever reaches a point is there: a function going back to
        // one skips every other on its way.
        self.constant(point as u64);
        self.emit(&Instruction::LocalSet(self.at));

        self.op(op);
        self.emit(&Instruction::LocalTee(self.scratch));
        self.emit(&Instruction::I64Eqz);
        self.emit(&Instruction::If(BlockType::Empty));
        self.frames.push(false);
        // It unwound. If the task is suspending, the function leaves at
        // once for where it keeps its locals: past every construct open
        // around this one is its outermost block.
        self.mode();
        self.constant(SUSPENDING);
        self.emit(&Instruction::I64Eq);
        let depth = u32::try_from(self.frames.len()).unwrap_or(0);
        self.emit(&Instruction::BrIf(depth));
        self.emit(&Instruction::I64Const(0));
        self.emit(&Instruction::Br(self.unwind_depth()));
        self.frames.pop();
        self.emit(&Instruction::End);
        self.emit(&Instruction::LocalGet(self.scratch));
    }

    /// What every point leaves for to suspend: the function keeps where
    /// it is and its locals, and returns. Every point shares it, so a
    /// function's code grows with its points and not with their product
    /// with its locals.
    pub(super) fn save(&mut self) {
        self.emit(&Instruction::LocalGet(CONTEXT));
        self.emit(&Instruction::LocalGet(self.at));
        self.runtime(Entry::FramePush);
        for local in self.saved() {
            self.local(local);
            self.runtime(Entry::FrameSave);
        }
        self.emit(&Instruction::Drop);
        self.emit(&Instruction::I64Const(0));
    }
}

//! The code that lets a task suspend in a function and resume in it. See
//! "Suspension" in `spec/backends.md`.

use ambient_runtime::{Context, Entry, RESUMING, SUSPENDING};
use wasm_encoder::{BlockType, Instruction};

use super::emit::{CONTEXT, Emitter, PARAMS, STACK, memory};
use super::import;
use crate::lir::{self, Op};

impl Emitter {
    /// Pushes the context's mode.
    pub(super) fn mode(&mut self) {
        self.emit(&Instruction::LocalGet(CONTEXT));
        self.emit(&Instruction::I64Load(memory(Context::MODE)));
    }

    /// Calls an entry point that takes the context and what is on the
    /// stack after it.
    pub(super) fn frame(&mut self, entry: Entry) {
        self.emit(&Instruction::Call(import(entry)));
    }

    /// What a function does first: it makes room for its locals, and if
    /// its task is resuming, it takes them back and notes where it was.
    pub(super) fn enter(&mut self) {
        self.emit(&Instruction::GlobalGet(STACK));
        self.emit(&Instruction::I32Const(self.size));
        self.emit(&Instruction::I32Sub);
        self.emit(&Instruction::LocalTee(self.frame));
        self.emit(&Instruction::GlobalSet(STACK));
        self.emit(&Instruction::I64Const(-1));
        self.emit(&Instruction::LocalSet(self.target));
        self.mode();
        self.constant(RESUMING);
        self.emit(&Instruction::I64Eq);
        self.emit(&Instruction::If(BlockType::Empty));
        self.emit(&Instruction::LocalGet(CONTEXT));
        self.frame(Entry::FramePoint);
        self.emit(&Instruction::LocalSet(self.target));
        for local in 0..self.locals {
            self.emit(&Instruction::LocalGet(CONTEXT));
            self.constant(u64::from(local));
            self.frame(Entry::FrameLoad);
            self.set_local(local as usize);
        }
        self.emit(&Instruction::LocalGet(CONTEXT));
        self.frame(Entry::FrameDrop);
        self.emit(&Instruction::Else);
        // Its parameters are the first of its locals.
        for param in 0..self.params {
            self.emit(&Instruction::LocalGet(PARAMS + param));
            self.set_local(param as usize);
        }
        self.emit(&Instruction::End);
    }

    /// Pushes whether the point being resumed to is one of `points`:
    /// first, and one past last.
    pub(super) fn resuming_to(&mut self, points: (usize, usize)) {
        self.emit(&Instruction::LocalGet(self.target));
        self.constant(points.0 as u64);
        self.emit(&Instruction::I64GeS);
        self.emit(&Instruction::LocalGet(self.target));
        self.constant(points.1 as u64);
        self.emit(&Instruction::I64LtS);
        self.emit(&Instruction::I32And);
    }

    /// An operation the task may suspend in. Leaves one word on the
    /// stack.
    pub(super) fn point(&mut self, point: usize, op: &Op, frame: Option<(usize, usize)>) {
        // A function that was going back to this point is there.
        self.resuming_to((point, point + 1));
        self.emit(&Instruction::If(BlockType::Empty));
        self.emit(&Instruction::I64Const(-1));
        self.emit(&Instruction::LocalSet(self.target));
        self.emit(&Instruction::End);

        self.op(op);
        self.emit(&Instruction::LocalTee(self.scratch));
        self.emit(&Instruction::I64Eqz);
        self.emit(&Instruction::If(BlockType::Empty));
        self.frames.push(false);
        // It unwound. If the task is suspending, this function keeps its
        // locals for later and leaves at once; if not, it unwinds.
        self.mode();
        self.constant(SUSPENDING);
        self.emit(&Instruction::I64Eq);
        self.emit(&Instruction::If(BlockType::Empty));
        self.constant(point as u64);
        self.emit(&Instruction::LocalSet(self.saved));
        let (definition, position) = match frame {
            Some((definition, position)) => (lir::Value::Constant(definition), position),
            None => (lir::Value::Null, 0),
        };
        self.value(definition);
        self.emit(&Instruction::LocalSet(self.saved + 1));
        self.constant(position as u64);
        self.emit(&Instruction::LocalSet(self.saved + 2));
        // Past this `if` and every construct open around it is the
        // function's outermost block.
        let depth = u32::try_from(self.frames.len()).unwrap_or(0) + 1;
        self.emit(&Instruction::Br(depth));
        self.emit(&Instruction::End);
        self.emit(&Instruction::I64Const(0));
        self.emit(&Instruction::Br(self.unwind_depth()));
        self.frames.pop();
        self.emit(&Instruction::End);
        self.emit(&Instruction::LocalGet(self.scratch));
    }

    /// What every point leaves for to suspend: the function keeps its
    /// frame and its locals, and returns. Every point shares it, so a
    /// function's code grows with its points and not with their product
    /// with its locals.
    pub(super) fn save(&mut self) {
        self.emit(&Instruction::LocalGet(CONTEXT));
        for word in 0..3 {
            self.emit(&Instruction::LocalGet(self.saved + word));
        }
        self.frame(Entry::FramePush);
        for local in 0..self.locals {
            self.emit(&Instruction::LocalGet(CONTEXT));
            self.local(local as usize);
            self.frame(Entry::FrameSave);
        }
        self.emit(&Instruction::I64Const(0));
    }
}

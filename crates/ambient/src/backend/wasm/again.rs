//! A function that starts again: what a call of itself that is the last
//! thing it does comes to. See [`Expr::Loop`].

use ambient_runtime::Entry;
use wasm_encoder::{BlockType, Instruction, ValType};

use super::emit::Emitter;
use crate::lir::{self, Expr};

impl Emitter {
    /// `body`, again for each turn an [`lir::Op::Again`] in it asks
    /// for. Leaves one word on the stack.
    pub(super) fn looped(&mut self, body: &Expr, held: usize) {
        let params = self.params as usize;
        let next = held + params;
        self.held = held;
        self.seen.turns = true;
        // The loop holds nothing yet, which memory does not say of
        // itself: unless the function is resuming, and has its locals
        // back. A function is at no point until it reaches its loop.
        if self.stale() {
            if self.resumes {
                self.before(0);
                self.emit(&Instruction::If(BlockType::Empty));
            }
            for local in held..=held + 2 * params {
                self.emit(&Instruction::I64Const(0));
                self.set_local(local);
            }
            if self.resumes {
                self.emit(&Instruction::End);
            }
        }
        self.emit(&Instruction::Loop(BlockType::Result(ValType::I64)));
        self.frames.push(false);
        // Where the body's unwinding lands.
        self.emit(&Instruction::Block(BlockType::Result(ValType::I64)));
        self.frames.push(true);
        self.expr(body);
        self.frames.pop();
        self.emit(&Instruction::End);

        // If the body unwound, that was for a turn if one is waiting.
        self.emit(&Instruction::LocalSet(self.scratch));
        self.local(next + params);
        self.emit(&Instruction::I64Const(0));
        self.emit(&Instruction::I64Ne);
        self.emit(&Instruction::If(BlockType::Empty));
        self.emit(&Instruction::I64Const(0));
        self.set_local(next + params);
        for param in 0..params {
            // One that was handed on as it is has nothing waiting.
            self.local(next + param);
            self.emit(&Instruction::I64Const(0));
            self.emit(&Instruction::I64Ne);
            self.emit(&Instruction::If(BlockType::Empty));
            self.let_go(held + param);
            for to in [held + param, param] {
                self.local(next + param);
                self.set_local(to);
            }
            self.emit(&Instruction::I64Const(0));
            self.set_local(next + param);
            self.emit(&Instruction::End);
        }
        self.nowhere();
        self.emit(&Instruction::Br(1));
        self.emit(&Instruction::End);
        self.emit(&Instruction::LocalGet(self.scratch));
        self.frames.pop();
        self.emit(&Instruction::End);

        // The function is over: what the last turn was given goes.
        for param in 0..params {
            self.let_go(held + param);
        }
    }

    /// Asks for a turn with `args` as the function's parameters, and
    /// leaves for the loop.
    pub(super) fn again(&mut self, args: &[lir::Value]) {
        let next = self.held + self.params as usize;
        for (param, arg) in args.iter().enumerate() {
            if *arg == lir::Value::Local(param) {
                continue;
            }
            self.value(*arg);
            self.runtime(Entry::Retain);
            self.value(*arg);
            self.set_local(next + param);
        }
        self.emit(&Instruction::I64Const(1));
        self.set_local(next + self.params as usize);
        self.emit(&Instruction::I64Const(0));
        self.emit(&Instruction::Br(self.unwind_depth()));
    }

    /// Gives up the reference a loop holds in `local`, if it holds
    /// one: a local it has not filled is zero.
    fn let_go(&mut self, local: usize) {
        self.local(local);
        self.emit(&Instruction::I64Const(0));
        self.emit(&Instruction::I64Ne);
        self.emit(&Instruction::If(BlockType::Empty));
        self.local(local);
        self.runtime(Entry::Release);
        self.emit(&Instruction::End);
    }
}

//! A function that starts again: what a call of itself that is the last
//! thing it does comes to. See [`Expr::Loop`].

use ambient_runtime::Entry;
use cranelift_codegen::ir::types::I64;
use cranelift_codegen::ir::{BlockArg, InstBuilder};

use super::{Emitter, Val};
use crate::lir::{self, Expr};

impl Emitter<'_> {
    /// `body`, again for each turn an [`lir::Op::Again`] in it asks
    /// for.
    pub(super) fn looped(&mut self, body: &Expr, held: usize) -> Val {
        let (start, landing, turn, out) = (
            self.builder.create_block(),
            self.builder.create_block(),
            self.builder.create_block(),
            self.builder.create_block(),
        );
        self.builder.append_block_param(landing, I64);
        self.builder.append_block_param(out, I64);
        self.held = held;
        self.builder.ins().jump(start, &[]);
        self.builder.switch_to_block(start);

        // However the body ends, it ends here: a `Let` hands on what
        // unwinds through it as it does a value.
        self.unwind.push(landing);
        let result = self.expr(body);
        self.unwind.pop();
        self.builder.ins().jump(landing, &[BlockArg::Value(result)]);

        // If it unwound, that was for a turn if one is waiting.
        self.builder.switch_to_block(landing);
        let result = self.builder.block_params(landing)[0];
        let next = held + self.params;
        let asked = next + self.params;
        let waiting = self.local(asked);
        let ended = [BlockArg::Value(result)];
        self.builder.ins().brif(waiting, turn, &[], out, &ended);

        self.builder.switch_to_block(turn);
        let zero = self.builder.ins().iconst(I64, 0);
        self.set_local(asked, zero);
        for param in 0..self.params {
            // One that was handed on as it is has nothing waiting.
            let (changed, same) = (self.builder.create_block(), self.builder.create_block());
            let new = self.local(next + param);
            self.builder.ins().brif(new, changed, &[], same, &[]);
            self.builder.switch_to_block(changed);
            let old = self.local(held + param);
            self.set_local(held + param, new);
            self.set_local(param, new);
            self.set_local(next + param, zero);
            self.let_go(old);
            self.builder.ins().jump(same, &[]);
            self.builder.switch_to_block(same);
        }
        self.builder.ins().jump(start, &[]);

        // The function is over: what the last turn was given goes.
        self.builder.switch_to_block(out);
        for param in 0..self.params {
            let old = self.local(held + param);
            self.let_go(old);
        }
        self.builder.block_params(out)[0]
    }

    /// Asks for a turn with `args` as the function's parameters, and
    /// leaves for the loop.
    pub(super) fn again(&mut self, args: &[lir::Value]) -> Val {
        let next = self.held + self.params;
        for (param, arg) in args.iter().enumerate() {
            if *arg == lir::Value::Local(param) {
                continue;
            }
            let word = self.value(*arg);
            self.count(Entry::Retain, word);
            self.set_local(next + param, word);
        }
        let one = self.builder.ins().iconst(I64, 1);
        self.set_local(next + self.params, one);
        let zero = self.builder.ins().iconst(I64, 0);
        if let Some(target) = self.unwind.last().copied() {
            self.builder.ins().jump(target, &[BlockArg::Value(zero)]);
        }
        // Nothing comes after a turn is asked for.
        let after = self.builder.create_block();
        self.builder.switch_to_block(after);
        zero
    }

    /// Gives up a reference a loop held, if it held one: a local it has
    /// not filled is zero.
    fn let_go(&mut self, word: Val) {
        let (held, done) = (self.builder.create_block(), self.builder.create_block());
        self.builder.ins().brif(word, held, &[], done, &[]);
        self.builder.switch_to_block(held);
        self.count(Entry::Release, word);
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(done);
    }
}

//! The code that lets a task suspend in a function and resume in it. See
//! "Suspension" in `spec/backends.md`.

use ambient_runtime::{Context, Entry, RESUMING, SUSPENDING};
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types::I64;
use cranelift_codegen::ir::{Block, BlockArg, InstBuilder};

use super::{Emitter, Val};
use crate::lir::{self, Expr, Op};

impl Emitter<'_> {
    /// What a function does first: if its task is resuming, it takes its
    /// locals back and notes where it was. One that has no point
    /// (`resumes`) is never resumed.
    pub(super) fn enter(&mut self, resumes: bool) {
        let running = self.builder.ins().iconst(I64, -1);
        self.builder.def_var(self.target, running);
        if !resumes {
            return;
        }
        let mode = self.load(self.context, Context::MODE);
        let resuming = self.is(mode, RESUMING);
        let (restore, start) = (self.builder.create_block(), self.builder.create_block());
        self.builder.ins().brif(resuming, restore, &[], start, &[]);

        self.builder.switch_to_block(restore);
        let point = self.entry(Entry::FramePoint, &[]);
        self.builder.def_var(self.target, point);
        for index in 0..self.locals {
            let index_word = self.constant(index as u64);
            let word = self.entry(Entry::FrameLoad, &[index_word]);
            self.set_local(index, word);
        }
        self.runtime(Entry::FrameDrop, &[]);
        self.builder.ins().jump(start, &[]);
        self.builder.switch_to_block(start);
    }

    /// Whether the point being resumed to is one of `points`: first, and
    /// one past last.
    pub(super) fn resuming_to(&mut self, points: (usize, usize)) -> Val {
        let target = self.builder.use_var(self.target);
        let (from, to) = (points.0 as u64, points.1 as u64);
        let ins = self.builder.ins();
        let reached = ins.icmp_imm_s(IntCC::SignedGreaterThanOrEqual, target, from.cast_signed());
        let within = self
            .builder
            .ins()
            .icmp_imm_s(IntCC::SignedLessThan, target, to.cast_signed());
        self.builder.ins().band(reached, within)
    }

    /// An operation the task may suspend in.
    pub(super) fn point(&mut self, point: usize, op: &Op, frame: Option<(usize, usize)>) -> Val {
        // A function that was going back to this point is there.
        let here = self.resuming_to((point, point + 1));
        let target = self.builder.use_var(self.target);
        let running = self.builder.ins().iconst(I64, -1);
        let next = self.builder.ins().select(here, running, target);
        self.builder.def_var(self.target, next);

        let word = self.op(op);
        let (proceed, unwound) = (self.builder.create_block(), self.builder.create_block());
        self.builder.ins().brif(word, proceed, &[], unwound, &[]);

        // It unwound. If the task is suspending, this function keeps its
        // locals for later and leaves at once; if not, it unwinds.
        self.builder.switch_to_block(unwound);
        let zero = self.builder.ins().iconst(I64, 0);
        let mode = self.load(self.context, Context::MODE);
        let suspending = self.is(mode, SUSPENDING);
        let unwinding = self.unwind.last().copied().unwrap_or(proceed);
        let (definition, position) = match frame {
            Some((definition, position)) => (lir::Value::Constant(definition), position),
            None => (lir::Value::Null, 0),
        };
        let at = [point as u64, position as u64].map(|word| self.constant(word));
        let definition = self.value(definition);
        let saved = [at[0], definition, at[1]].map(BlockArg::Value);
        let save = self.save_block();
        let passed = [BlockArg::Value(zero)];
        self.builder
            .ins()
            .brif(suspending, save, &saved, unwinding, &passed);

        self.builder.switch_to_block(proceed);
        word
    }

    /// The block that keeps the locals of a function whose task is
    /// suspending. It takes the point the function is at, and the
    /// definition and position of its frame. Every point shares it, so a
    /// function's code grows with its points and not with their product
    /// with its locals.
    fn save_block(&mut self) -> Block {
        if let Some(save) = self.save {
            return save;
        }
        let save = self.builder.create_block();
        for _ in 0..3 {
            self.builder.append_block_param(save, I64);
        }
        self.save = Some(save);
        save
    }

    /// Emits the block every point leaves through to suspend, if any
    /// does.
    pub(super) fn save(&mut self) {
        let Some(save) = self.save else {
            return;
        };
        self.builder.switch_to_block(save);
        let frame = self.builder.block_params(save).to_vec();
        self.runtime(Entry::FramePush, &frame);
        for index in 0..self.locals {
            let word = self.local(index);
            self.runtime(Entry::FrameSave, &[word]);
        }
        let zero = self.builder.ins().iconst(I64, 0);
        self.builder.ins().return_(&[zero]);
    }

    /// The value a `Let` binds, in a function that may be resuming.
    pub(super) fn resumed(&mut self, points: (usize, usize), local: usize, value: &Expr) -> Val {
        // Unless the function is on its way to a point elsewhere,
        // in which case the local has its value already.
        let target = self.builder.use_var(self.target);
        let running = self
            .builder
            .ins()
            .icmp_imm_s(IntCC::SignedLessThan, target, 0);
        let within = self.resuming_to(points);
        let compute = self.builder.ins().bor(running, within);
        let (run, skip, join) = (
            self.builder.create_block(),
            self.builder.create_block(),
            self.builder.create_block(),
        );
        self.builder.append_block_param(join, I64);
        self.builder.ins().brif(compute, run, &[], skip, &[]);
        self.builder.switch_to_block(run);
        let word = self.expr(value);
        self.builder.ins().jump(join, &[BlockArg::Value(word)]);
        self.builder.switch_to_block(skip);
        let word = self.local(local);
        self.builder.ins().jump(join, &[BlockArg::Value(word)]);
        self.builder.switch_to_block(join);
        self.builder.block_params(join)[0]
    }
}

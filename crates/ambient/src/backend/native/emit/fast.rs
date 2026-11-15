//! What is answered without calling the runtime: the common case of an
//! operation, tested for inline.
//!
//! The runtime's entry stays what the operation means. Each case here
//! gives exactly what the entry would, for the words it takes, and
//! calls the entry for every other.

use ambient_runtime::{Entry, FALSE, TRUE};
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types::I64;
use cranelift_codegen::ir::{BlockArg, InstBuilder};

use super::{Emitter, Val};
use crate::core::Primitive;
use crate::lir;

/// The bits of a word that say what it is: none are set in an object's
/// address, and the lowest is set in an immediate integer.
const TAG: i64 = 0b111;

impl Emitter<'_> {
    /// Takes or gives up a reference, with [`Entry::Retain`] or
    /// [`Entry::Release`]: only an object is counted, so the entry is
    /// called for nothing else.
    pub(super) fn count(&mut self, entry: Entry, word: Val) {
        let (counted, done) = (self.builder.create_block(), self.builder.create_block());
        let tag = self.builder.ins().band_imm_s(word, TAG);
        self.builder.ins().brif(tag, done, &[], counted, &[]);
        self.builder.switch_to_block(counted);
        self.runtime(entry, &[word]);
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(done);
    }

    /// An operation that has a case answered inline, or nothing if it
    /// has none.
    pub(super) fn fast(&mut self, operation: Primitive, args: &[lir::Value]) -> Option<Val> {
        let entry = match operation {
            Primitive::IntAdd => Entry::IntAdd,
            Primitive::IntSubtract => Entry::IntSubtract,
            Primitive::IntLess => Entry::IntLess,
            Primitive::Equal => Entry::Equal,
            _ => return None,
        };
        let [left, right] = args else {
            return None;
        };
        let (left, right) = (self.value(*left), self.value(*right));
        let (quick, slow, join) = (
            self.builder.create_block(),
            self.builder.create_block(),
            self.builder.create_block(),
        );
        self.builder.append_block_param(join, I64);
        if operation == Primitive::Equal {
            self.same(left, right, quick, slow, join);
        } else {
            self.small(operation, left, right, quick, slow, join);
        }
        self.builder.switch_to_block(slow);
        let result = self.entry(entry, &[left, right]);
        self.builder.ins().jump(join, &[BlockArg::Value(result)]);
        self.builder.switch_to_block(join);
        Some(self.builder.block_params(join)[0])
    }

    /// Integers that are both immediate: the sum or difference if it is
    /// immediate too, or which is less.
    fn small(
        &mut self,
        operation: Primitive,
        left: Val,
        right: Val,
        quick: cranelift_codegen::ir::Block,
        slow: cranelift_codegen::ir::Block,
        join: cranelift_codegen::ir::Block,
    ) {
        let both = self.builder.ins().band(left, right);
        let both = self.builder.ins().band_imm_s(both, 1);
        self.builder.ins().brif(both, quick, &[], slow, &[]);
        self.builder.switch_to_block(quick);
        if operation == Primitive::IntLess {
            let less = self.builder.ins().icmp(IntCC::SignedLessThan, left, right);
            let result = self.truth(less);
            self.builder.ins().jump(join, &[BlockArg::Value(result)]);
            return;
        }
        // An immediate is twice the integer and one: with the one taken
        // off the right, the sum of two is the sum as an immediate, and
        // leaves the word exactly when the sum is no immediate.
        let untagged = self.builder.ins().iadd_imm_s(right, -1);
        let (result, overflowed) = if operation == Primitive::IntAdd {
            self.builder.ins().sadd_overflow(left, untagged)
        } else {
            self.builder.ins().ssub_overflow(left, untagged)
        };
        self.builder
            .ins()
            .brif(overflowed, slow, &[], join, &[BlockArg::Value(result)]);
    }

    /// Equality of words that are the same, or of which one is no
    /// object: only two objects are looked into.
    fn same(
        &mut self,
        left: Val,
        right: Val,
        quick: cranelift_codegen::ir::Block,
        slow: cranelift_codegen::ir::Block,
        join: cranelift_codegen::ir::Block,
    ) {
        let same = self.builder.ins().icmp(IntCC::Equal, left, right);
        let yes = self.constant(TRUE);
        self.builder
            .ins()
            .brif(same, join, &[BlockArg::Value(yes)], quick, &[]);
        self.builder.switch_to_block(quick);
        let either = self.builder.ins().bor(left, right);
        let tag = self.builder.ins().band_imm_s(either, TAG);
        let no = self.constant(FALSE);
        self.builder
            .ins()
            .brif(tag, join, &[BlockArg::Value(no)], slow, &[]);
    }

    /// A condition as a boolean value.
    fn truth(&mut self, condition: Val) -> Val {
        let (yes, no) = (self.constant(TRUE), self.constant(FALSE));
        self.builder.ins().select(condition, yes, no)
    }
}

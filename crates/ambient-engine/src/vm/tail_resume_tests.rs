//! VM-level tests for the `TailResume` opcode.
//!
//! `TailResume` is `Resume` fused with the arm frame's `Return`: it discards
//! the arm frame before reinstating the captured continuation, so a handler
//! that resumes on every perform/resume cycle runs in constant frame space
//! instead of parking one arm frame per cycle. The deep-loop test drives far
//! past `max_call_depth` (1000); that it returns a value rather than
//! `VmError::StackOverflow` is the assertion that frame growth is bounded.

use crate::bytecode::Opcode;
use crate::test_utils::{FunctionBuilder, VmTest, test_method_ref, test_never_method_ref};
use crate::vm::VmError;

/// Iteration count deliberately far above `max_call_depth` (1000): a resuming
/// handler loop that parked one frame per cycle would overflow long before.
const DEEP: f64 = 100_000.0;

/// A resumable ability method taking no arguments (`tick()`), no default impl.
fn tick_method() -> crate::value::AbilityMethodRef {
    test_method_ref(7, 0, None)
}

/// `loop_perform(n, acc)`: `if n <= 0 { acc } else { let x = tick!();
/// loop_perform(n - 1, acc + x) }`. Self-tail-recursive, performing `tick`
/// every iteration; the recursive call is a `TailCall`.
fn loop_perform() -> crate::bytecode::CompiledFunction {
    let self_hash = blake3::hash(b"test::tail_resume_loop");
    let method = tick_method();
    FunctionBuilder::new("test::tail_resume_loop")
        .with_params(2)
        .with_locals(3)
        .with_builder(|b| {
            // n <= 0 ? return acc.
            b.emit_u16(Opcode::LoadLocal, 0);
            b.emit_const(crate::value::Value::Number(0.0));
            b.emit(Opcode::Le);
            let recurse = b.emit_jump_placeholder(Opcode::JumpIfNot);
            b.emit_u16(Opcode::LoadLocal, 1);
            b.emit(Opcode::Return);
            // recurse: loop_perform(n - 1, acc + tick!())
            b.patch_jump(recurse);
            b.emit_u16(Opcode::LoadLocal, 0); // n
            b.emit_const(crate::value::Value::Number(1.0));
            b.emit(Opcode::Sub); // n - 1  (arg0)
            b.emit_u16(Opcode::LoadLocal, 1); // acc
            b.emit_suspend(method.clone(), 0);
            b.emit(Opcode::Perform); // x = tick!()
            b.emit(Opcode::Add); // acc + x  (arg1)
            b.emit_tail_call(self_hash, 2);
        })
        .build()
}

/// A `harness(n, acc)` frame that installs the resuming handler for `tick`
/// and tail-calls `loop_perform`. Installing the handler here (rather than on
/// the entry frame) puts the handler boundary strictly above the entry
/// frame, mirroring the real compiler's handle-thunk: when the arm frame is
/// popped by `TailResume`, the entry frame always remains beneath it.
fn harness(loop_hash: blake3::Hash, arm_hash: blake3::Hash) -> crate::bytecode::CompiledFunction {
    let method = tick_method();
    FunctionBuilder::new("test::tail_resume_harness")
        .with_params(2)
        .with_locals(2)
        .with_builder(move |b| {
            b.emit_make_handler(method.ability_id, &[(method.clone(), arm_hash)], 0);
            b.emit_handle_with_value();
            b.emit_u16(Opcode::LoadLocal, 0); // n
            b.emit_u16(Opcode::LoadLocal, 1); // acc
            b.emit_tail_call(loop_hash, 2);
        })
        .build()
}

#[test]
fn tail_resuming_handler_loop_runs_in_constant_frame_space() {
    // The resuming handler drives `loop_perform` for `DEEP` cycles. Each
    // `tick!()` is caught, the arm tail-resumes with 1, and the tail resume
    // discards the arm frame before reinstating the continuation — so the
    // frame count never grows past {entry, loop_perform}. Before `TailResume`
    // this overflowed at `max_call_depth`. Every cycle adds 1 to the
    // accumulator, so `loop_perform(DEEP, 0)` returns `DEEP`.
    let arm = FunctionBuilder::new("test::resume_one_arm")
        .with_params(2)
        .with_locals(2)
        .load_local(0) // continuation
        .push(1.0) // resume value
        .tail_resume()
        .build();
    let arm_hash = arm.hash;

    let looper = loop_perform();
    let loop_hash = looper.hash;
    let harness = harness(loop_hash, arm_hash);
    let harness_hash = harness.hash;

    VmTest::new()
        .with_function(arm)
        .with_function(looper)
        .with_function(harness)
        .push(DEEP) // n
        .push(0.0) // acc
        .call_func(harness_hash, 2)
        .expect_number(DEEP);
}

#[test]
fn tail_resume_on_unit_continuation_errors_like_resume() {
    // A never method's arm receives unit in the continuation slot (no
    // continuation was captured). Tail-resuming it must fail loudly, exactly
    // as `Resume` would — the never-arm backstop if bytecode disagrees with
    // the checker's ban on `resume` in never arms.
    let arm = FunctionBuilder::new("test::never_tail_resume_arm")
        .with_params(2)
        .with_locals(2)
        .load_local(0) // unit continuation slot
        .push(1.0)
        .tail_resume()
        .build();
    let arm_hash = arm.hash;
    let method = test_never_method_ref(3, 0);

    VmTest::new()
        .with_function(arm)
        .handle(&method, arm_hash)
        .push(5.0)
        .suspend(&method, 1)
        .perform()
        .expect_error(VmError::ExpectedContinuation { got: "unit" });
}

#[test]
fn tail_resume_after_a_non_tail_resume_of_the_same_continuation_errors() {
    // Single-shot enforcement still holds across opcodes: an arm that
    // `Resume`s (non-tail) and then `TailResume`s the *same* continuation
    // trips `ContinuationAlreadyResumed` on the second use — the CAS is
    // shared by both opcodes and fires before the arm frame is popped.
    let arm = FunctionBuilder::new("test::resume_then_tail_resume")
        .with_params(2)
        .with_locals(2)
        .with_builder(|b| {
            b.emit_u16(Opcode::LoadLocal, 0); // continuation
            b.emit_const(crate::value::Value::Number(1.0));
            b.emit(Opcode::Resume); // first (non-tail) resume
            b.emit(Opcode::Pop); // discard the resumed region's result
            b.emit_u16(Opcode::LoadLocal, 0); // the same continuation
            b.emit_const(crate::value::Value::Number(2.0));
            b.emit(Opcode::TailResume); // single-shot violation
        })
        .build();
    let arm_hash = arm.hash;
    // Resumable method with a default impl so the one perform below has a
    // continuation to capture and the resumed region simply returns.
    let method = test_method_ref(2, 0, None);

    VmTest::new()
        .with_function(arm)
        .handle(&method, arm_hash)
        .push(5.0)
        .suspend(&method, 1)
        .perform()
        .expect_error(VmError::ContinuationAlreadyResumed);
}

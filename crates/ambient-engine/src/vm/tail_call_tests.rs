//! VM-level tests for the `TailCall` / `TailCallClosure` opcodes.
//!
//! Every function here is hand-assembled through `FunctionBuilder`, whose
//! `build` overrides the content hash to `blake3(name)` — so a function can
//! reference itself by tail-calling `blake3(name)`. The deep-recursion tests
//! iterate far past `max_call_depth` (1000); that they return a value rather
//! than `VmError::StackOverflow` is the assertion that the frame count never
//! grows.

use std::sync::Arc;

use crate::bytecode::Opcode;
use crate::test_utils::{FunctionBuilder, VmTest, test_method_ref};
use crate::value::Value;
use crate::vm::{Vm, VmError};

/// Iteration count deliberately far above `max_call_depth` (1000): a
/// non-tail recursion this deep would overflow the call stack.
const DEEP: f64 = 100_000.0;

/// The triangular sum `1 + 2 + ... + n`, the value the tail-recursive
/// accumulator functions below compute for `sum(n, 0)`.
fn triangular(n: f64) -> f64 {
    n * (n + 1.0) / 2.0
}

/// `sum(n, acc)`: `if n <= 0 { acc } else { sum(n - 1, acc + n) }`, a
/// self-tail-recursive accumulator via `TailCall`.
fn tail_sum() -> crate::bytecode::CompiledFunction {
    let self_hash = blake3::hash(b"test::tail_sum");
    FunctionBuilder::new("test::tail_sum")
        .with_params(2)
        .with_locals(2)
        .with_builder(|b| {
            // n <= 0 ?
            b.emit_u16(Opcode::LoadLocal, 0);
            b.emit_const(Value::Number(0.0));
            b.emit(Opcode::Le);
            let recurse = b.emit_jump_placeholder(Opcode::JumpIfNot);
            // base: return acc
            b.emit_u16(Opcode::LoadLocal, 1);
            b.emit(Opcode::Return);
            // recurse: tail_sum(n - 1, acc + n)
            b.patch_jump(recurse);
            b.emit_u16(Opcode::LoadLocal, 0); // n
            b.emit_const(Value::Number(1.0));
            b.emit(Opcode::Sub); // n - 1  (arg0)
            b.emit_u16(Opcode::LoadLocal, 1); // acc
            b.emit_u16(Opcode::LoadLocal, 0); // n
            b.emit(Opcode::Add); // acc + n (arg1)
            b.emit_tail_call(self_hash, 2);
        })
        .build()
}

#[test]
fn tail_call_recurses_in_constant_stack_space() {
    let f = tail_sum();
    let hash = f.hash;

    VmTest::new()
        .with_function(f)
        .push(DEEP)
        .push(0.0)
        .call_func(hash, 2)
        .expect_number(triangular(DEEP));
}

#[test]
fn non_tail_recursion_still_overflows_at_depth_1000() {
    // Unbounded self-`Call` (no tail): each call pushes a frame, so the
    // depth guard trips. This is the behavior tail calls deliberately avoid.
    let spin = FunctionBuilder::new("test::deep_call")
        .with_builder(|b| {
            let self_hash = blake3::hash(b"test::deep_call");
            b.emit_call(self_hash, 0);
        })
        .build();
    let hash = spin.hash;

    let mut vm = Vm::new();
    vm.load_function(spin);
    assert_eq!(vm.call(&hash, Vec::new()), Err(VmError::StackOverflow));
}

#[test]
fn tail_call_closure_with_bare_function_ref_callee() {
    // `TailCallClosure` accepts a plain `FunctionRef` callee (no closure
    // environment), reusing the frame just like `TailCall`.
    let self_hash = blake3::hash(b"test::tail_sum_via_ref");
    let f = FunctionBuilder::new("test::tail_sum_via_ref")
        .with_params(2)
        .with_locals(2)
        .with_builder(|b| {
            b.emit_u16(Opcode::LoadLocal, 0);
            b.emit_const(Value::Number(0.0));
            b.emit(Opcode::Le);
            let recurse = b.emit_jump_placeholder(Opcode::JumpIfNot);
            b.emit_u16(Opcode::LoadLocal, 1);
            b.emit(Opcode::Return);
            b.patch_jump(recurse);
            // Callee first (a bare function ref), then the two args.
            b.emit_const(Value::FunctionRef(self_hash));
            b.emit_u16(Opcode::LoadLocal, 0);
            b.emit_const(Value::Number(1.0));
            b.emit(Opcode::Sub);
            b.emit_u16(Opcode::LoadLocal, 1);
            b.emit_u16(Opcode::LoadLocal, 0);
            b.emit(Opcode::Add);
            b.emit_tail_call_closure(2);
        })
        .build();
    let hash = f.hash;

    VmTest::new()
        .with_function(f)
        .push(DEEP)
        .push(0.0)
        .call_func(hash, 2)
        .expect_number(triangular(DEEP));
}

#[test]
fn tail_call_closure_replaces_captures_each_iteration() {
    // A closure captures the running accumulator and takes `n` as its one
    // argument. Each iteration builds a fresh closure whose capture is
    // `acc + n` and tail-calls it with `n - 1`, so frame reuse must install
    // the new closure's environment every time.
    let self_hash = blake3::hash(b"test::tail_sum_closure");
    let f = FunctionBuilder::new("test::tail_sum_closure")
        .with_params(1) // n
        .with_locals(1)
        .with_builder(|b| {
            // n <= 0 ? return the captured acc.
            b.emit_u16(Opcode::LoadLocal, 0);
            b.emit_const(Value::Number(0.0));
            b.emit(Opcode::Le);
            let recurse = b.emit_jump_placeholder(Opcode::JumpIfNot);
            b.emit_load_capture(0); // acc
            b.emit(Opcode::Return);
            // recurse: closure = make(self, [acc + n]); tail (closure)(n - 1)
            b.patch_jump(recurse);
            b.emit_load_capture(0); // acc
            b.emit_u16(Opcode::LoadLocal, 0); // n
            b.emit(Opcode::Add); // acc + n  (the new capture)
            b.emit_make_closure(self_hash, 1); // callee closure on the stack
            b.emit_u16(Opcode::LoadLocal, 0); // n
            b.emit_const(Value::Number(1.0));
            b.emit(Opcode::Sub); // n - 1  (arg)
            b.emit_tail_call_closure(1);
        })
        .build();
    let hash = f.hash;

    let mut vm = Vm::new();
    vm.load_function(f);
    // Start with acc = 0 captured, n = DEEP.
    let result = vm.call_closure(&hash, vec![Value::Number(DEEP)], vec![Value::Number(0.0)]);
    assert_eq!(result, Ok(Value::Number(triangular(DEEP))));
}

#[test]
fn tail_call_to_native_returns_result_to_original_caller() {
    // `inner` tail-calls a native `double`; the native pushes no frame, so
    // its result unwinds `inner` and flows back to `outer`, which called
    // `inner` with a normal `Call`.
    let double_hash = blake3::hash(b"test::double");
    let inner = FunctionBuilder::new("test::inner")
        .with_builder(|b| {
            b.emit_const(Value::Number(21.0));
            b.emit_tail_call(double_hash, 1);
        })
        .build();
    let inner_hash = inner.hash;

    VmTest::new()
        .with_native(
            "test::double",
            1,
            Arc::new(|args| Ok(Value::Number(args[0].as_number().unwrap() * 2.0))),
        )
        .with_function(inner)
        .call_func(inner_hash, 0) // outer calls inner normally
        .expect_number(42.0);
}

#[test]
fn tail_called_function_can_still_grow_the_stack_with_a_normal_call() {
    // A function reached by a tail call does an ordinary `Call`, which
    // pushes a frame from the reused frame as usual.
    let helper = FunctionBuilder::new("test::helper").push(10.0).build();
    let helper_hash = helper.hash;

    let tail_target = FunctionBuilder::new("test::grows")
        .with_builder(|b| {
            b.emit_call(helper_hash, 0); // normal call: depth grows
            b.emit_const(Value::Number(32.0));
            b.emit(Opcode::Add);
        })
        .build();
    let target_hash = tail_target.hash;

    let entry = FunctionBuilder::new("test::entry")
        .with_builder(|b| b.emit_tail_call(target_hash, 0))
        .build();
    let entry_hash = entry.hash;

    VmTest::new()
        .with_function(helper)
        .with_function(tail_target)
        .with_function(entry)
        .call_func(entry_hash, 0)
        .expect_number(42.0);
}

#[test]
fn tail_call_arity_mismatch_errors_like_call() {
    // A two-param callee tail-called with zero arguments faults with the
    // same `ArityMismatch` a normal `Call` raises.
    let callee = FunctionBuilder::new("test::needs_two")
        .with_params(2)
        .with_locals(2)
        .push(0.0)
        .build();
    let callee_hash = callee.hash;

    VmTest::new()
        .with_function(callee)
        .with_builder(|b| b.emit_tail_call(callee_hash, 0))
        .expect_error(VmError::ArityMismatch {
            expected: 2,
            got: 0,
        });
}

#[test]
fn tail_call_with_zero_args_from_a_frame_with_locals() {
    // argc == 0, and the caller frame owns a nonzero local footprint: all
    // of it must be discarded so the callee starts with an empty base.
    let target = FunctionBuilder::new("test::zero_arg_target")
        .push(99.0)
        .build();
    let target_hash = target.hash;

    let caller = FunctionBuilder::new("test::has_locals")
        .with_locals(3) // three Unit locals filled on entry
        .with_builder(|b| b.emit_tail_call(target_hash, 0))
        .build();
    let caller_hash = caller.hash;

    VmTest::new()
        .with_function(target)
        .with_function(caller)
        .call_func(caller_hash, 0)
        .expect_number(99.0);
}

#[test]
fn tail_call_with_more_args_than_the_callers_footprint() {
    // The caller has a one-slot footprint but tail-calls a three-arg
    // function: the arguments sit above the frame base and slide down
    // correctly even though there are more of them than the old locals.
    let target = FunctionBuilder::new("test::sum3")
        .with_params(3)
        .with_locals(3)
        .with_builder(|b| {
            b.emit_u16(Opcode::LoadLocal, 0);
            b.emit_u16(Opcode::LoadLocal, 1);
            b.emit(Opcode::Add);
            b.emit_u16(Opcode::LoadLocal, 2);
            b.emit(Opcode::Add);
        })
        .build();
    let target_hash = target.hash;

    let caller = FunctionBuilder::new("test::one_local")
        .with_params(1) // x, the sole local slot
        .with_locals(1)
        .with_builder(|b| {
            b.emit_const(Value::Number(10.0));
            b.emit_const(Value::Number(20.0));
            b.emit_const(Value::Number(30.0));
            b.emit_tail_call(target_hash, 3);
        })
        .build();
    let caller_hash = caller.hash;

    VmTest::new()
        .with_function(target)
        .with_function(caller)
        .push(7.0) // x, discarded by the tail call
        .call_func(caller_hash, 1)
        .expect_number(60.0);
}

#[test]
fn handler_survives_a_tail_call_into_the_delimited_region() {
    // The entry frame installs a handler, then tail-calls a performer. Frame
    // reuse leaves the handler installed (its boundary is the reused frame),
    // so the performer's perform is caught and resumed — the continuation is
    // captured and restored across the reused frame.
    let method = test_method_ref(2, 0, None); // resumable, no default impl
    let arm = FunctionBuilder::new("test::arm")
        .with_params(2)
        .with_locals(2)
        .load_local(0) // continuation
        .push(42.0) // resume value
        .resume()
        .build();
    let arm_hash = arm.hash;

    // performer: perform the method (no handler => it would be unhandled).
    let performer = FunctionBuilder::new("test::performer")
        .push(5.0)
        .suspend(&method, 1)
        .perform()
        .build();
    let performer_hash = performer.hash;

    VmTest::new()
        .with_function(arm)
        .with_function(performer)
        .handle(&method, arm_hash) // boundary = the entry frame
        .with_builder(|b| b.emit_tail_call(performer_hash, 0))
        .expect_number(42.0);
}

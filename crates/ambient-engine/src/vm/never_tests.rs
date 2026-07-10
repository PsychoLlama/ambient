//! Never-returning (`: !`) ability methods at the VM level.
//!
//! Performing a never method unwinds: the delimited computation is
//! discarded outright and no continuation is created — the arm's
//! continuation slot holds an inert unit. These tests drive the raw
//! opcodes, so the "delimited computation" is the performing frame
//! itself: the arm's return value becomes the run's final result.

use crate::test_utils::{FunctionBuilder, VmTest, test_method_ref, test_never_method_ref};
use crate::value::Value;

/// An arm that returns whatever sits in its continuation slot (local 0).
fn continuation_echo_arm() -> crate::bytecode::CompiledFunction {
    FunctionBuilder::new("test::echo_continuation")
        .with_locals(2)
        .with_params(2)
        .load_local(0)
        .build()
}

#[test]
fn test_never_perform_passes_no_continuation() {
    // A never method's arm receives unit in the continuation slot: the
    // delimited state was dropped at the perform, not captured.
    let arm = continuation_echo_arm();
    let arm_hash = arm.hash;
    let method = test_never_method_ref(3, 0);

    VmTest::new()
        .with_function(arm)
        .handle(&method, arm_hash)
        .push(5.0)
        .suspend(&method, 1)
        .perform()
        .expect_unit();
}

#[test]
fn test_ordinary_perform_still_captures_a_continuation() {
    // Control: the same shape with a resumable method hands the arm a
    // real continuation value.
    let arm = continuation_echo_arm();
    let arm_hash = arm.hash;
    let method = test_method_ref(3, 0, None);

    VmTest::new()
        .with_function(arm)
        .handle(&method, arm_hash)
        .push(5.0)
        .suspend(&method, 1)
        .perform()
        .expect_match(|r| matches!(r, Ok(Value::Continuation(_))));
}

#[test]
fn test_host_constructed_suspended_never_value_unwinds() {
    // A suspended value built host-side (not by the `Suspend` opcode) must
    // carry the declaration's unwind semantics: the sole constructor
    // derives `never` from the method reference, so performing it drops
    // the delimited state exactly like a compiled perform site would.
    let arm = continuation_echo_arm();
    let arm_hash = arm.hash;
    let method = test_never_method_ref(3, 0);
    let suspended = Value::SuspendedAbility(std::sync::Arc::new(
        ambient_ability::SuspendedAbility::from_method_ref(&method, vec![Value::Number(5.0)]),
    ));

    VmTest::new()
        .with_function(arm)
        .handle(&method, arm_hash)
        .push_value(suspended)
        .perform()
        .expect_unit();
}

#[test]
fn test_never_arm_value_is_the_completion_value() {
    // The arm's own value lands at the handle completion point; the
    // method's argument is still delivered through the suspended ability.
    let arm = FunctionBuilder::new("test::code_plus_one")
        .with_locals(3)
        .with_params(2)
        .with_builder(|b| {
            b.emit_u16(crate::bytecode::Opcode::LoadLocal, 1);
            b.emit_get_ability_arg(0);
        })
        .push(1.0)
        .add()
        .build();
    let arm_hash = arm.hash;
    let method = test_never_method_ref(3, 0);

    VmTest::new()
        .with_function(arm)
        .handle(&method, arm_hash)
        .push(41.0)
        .suspend(&method, 1)
        .perform()
        .expect_number(42.0);
}

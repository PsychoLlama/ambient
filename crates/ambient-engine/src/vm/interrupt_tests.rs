//! Host interrupts at native call sites, and the hard-stop flag.
//!
//! A blocking native interrupted by the host returns
//! [`VmError::Interrupted`]; the VM responds by performing the identified
//! abstract never method at the native's own call site (a
//! host-constructed suspended never value), so the unwind lands exactly
//! where the interrupted perform sits. The hard-stop flag is the backstop
//! for computations that never reach an interruptible native: the
//! execution loop aborts at its next periodic check.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::bytecode::Opcode;
use crate::test_utils::{FunctionBuilder, VmTest, test_never_method_ref};
use crate::value::{AbilityMethodRef, Value};
use crate::vm::{Vm, VmError};

/// The error an interrupted native returns for `method`.
fn interrupt_error(method: &AbilityMethodRef) -> VmError {
    VmError::Interrupted {
        ability_id: method.ability_id,
        method: method.method_key(),
    }
}

#[test]
fn test_interrupted_native_unwinds_to_the_covering_arm() {
    // The nearest covering handler arm runs with no continuation, and
    // its value is the completion value — exactly the semantics of a
    // compiled never perform at the native's call site.
    let arm = FunctionBuilder::new("test::cleanup")
        .with_locals(2)
        .with_params(2)
        .push(42.0)
        .build();
    let arm_hash = arm.hash;
    let method = test_never_method_ref(3, 0);
    let error = interrupt_error(&method);

    VmTest::new()
        .with_function(arm)
        .with_native(
            "test::blocking",
            0,
            Arc::new(move |_args| Err(error.clone())),
        )
        .handle(&method, arm_hash)
        .call_native("test::blocking", 0)
        .expect_number(42.0);
}

#[test]
fn test_interrupted_native_without_a_handler_is_an_unhandled_ability_fault() {
    // No covering arm and no default implementation (the interrupt is an
    // abstract never method): the fault surfaces to the driving host.
    let method = test_never_method_ref(3, 0);
    let error = interrupt_error(&method);
    let expected_ability = method.ability_id;
    let expected_method = method.method_key();

    VmTest::new()
        .with_native(
            "test::blocking",
            0,
            Arc::new(move |_args| Err(error.clone())),
        )
        .call_native("test::blocking", 0)
        .expect_match(|r| {
            matches!(
                r,
                Err(VmError::UnhandledAbility { ability_id, method })
                    if *ability_id == expected_ability && *method == expected_method
            )
        });
}

#[test]
fn test_work_before_the_interrupted_native_ran_to_completion() {
    // Straight-line code between interruptible natives always runs: the
    // unwind lands only at the interrupted call, never earlier.
    let ran = Arc::new(AtomicBool::new(false));
    let observer = Arc::clone(&ran);

    let arm = FunctionBuilder::new("test::cleanup")
        .with_locals(2)
        .with_params(2)
        .push(7.0)
        .build();
    let arm_hash = arm.hash;
    let method = test_never_method_ref(3, 0);
    let error = interrupt_error(&method);

    VmTest::new()
        .with_function(arm)
        .with_native(
            "test::effect",
            0,
            Arc::new(move |_args| {
                observer.store(true, Ordering::SeqCst);
                Ok(Value::Unit)
            }),
        )
        .with_native(
            "test::blocking",
            0,
            Arc::new(move |_args| Err(error.clone())),
        )
        .handle(&method, arm_hash)
        .call_native("test::effect", 0)
        .pop()
        .call_native("test::blocking", 0)
        .expect_number(7.0);

    assert!(
        ran.load(Ordering::SeqCst),
        "the effect before the interrupted native must have completed"
    );
}

#[test]
fn test_hard_stop_flag_stops_a_hot_loop() {
    // A genuine infinite loop (a backward jump over itself) with no
    // performs: only the periodic flag check can end it.
    let spin = FunctionBuilder::new("test::spin")
        .with_builder(|b| b.emit_i16(Opcode::Jump, -3))
        .build();
    let spin_hash = spin.hash;

    let mut vm = Vm::new();
    vm.load_function(spin);
    let flag = Arc::new(AtomicBool::new(false));
    vm.set_interrupt_flag(Arc::clone(&flag));

    let runner = std::thread::spawn(move || vm.call(&spin_hash, Vec::new()));
    std::thread::sleep(std::time::Duration::from_millis(20));
    flag.store(true, Ordering::SeqCst);

    let result = runner.join().expect("vm thread completes");
    assert_eq!(result, Err(VmError::HardStopped));
}

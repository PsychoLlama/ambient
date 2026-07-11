//! Reentrant invocation (`Vm::invoke`) from VM-invoking natives.
//!
//! A `NativeVmFn` receives the calling VM mid-execution and may run
//! function values on it. These tests pin the contract:
//!
//! - the nested callee runs to completion and the caller's frame resumes
//!   with the native's result, exactly like a pure native;
//! - ability dispatch inside the nested region is delimited at the invoke
//!   boundary — the caller's handlers are invisible, and performs fall
//!   through to default implementations;
//! - a failed nested call restores every VM height, so the caller can
//!   continue executing.

use std::sync::Arc;

use crate::test_utils::{FunctionBuilder, test_method_ref};
use crate::value::Value;
use crate::vm::Vm;

/// A fresh VM with `funcs` loaded and one VM-invoking native registered
/// under a synthetic hash/uuid pair. Returns the VM and the native's hash.
fn vm_with_native(
    funcs: Vec<crate::bytecode::CompiledFunction>,
    arity: u8,
    native: crate::natives::NativeVmFn,
) -> (Vm, blake3::Hash) {
    let uuid = uuid::Uuid::from_u128(0xDEAD_BEEF);
    let hash = blake3::hash(b"test::vm_native");
    let mut vm = Vm::new();
    for func in funcs {
        vm.load_function(func);
    }
    vm.load_native(hash, uuid, arity);
    vm.register_native_vm_impl(uuid, native);
    (vm, hash)
}

#[test]
fn test_native_invokes_a_function_mid_execution() {
    // outer → native(double, 5) → invoke runs double(5) on the same VM →
    // native returns 10+1 → outer continues (+100). The caller's frame
    // survives the nested execution untouched.
    let double = FunctionBuilder::new("test::double")
        .with_params(1)
        .with_locals(1)
        .load_local(0)
        .push(2.0)
        .mul()
        .build();
    let double_hash = double.hash;

    let (mut vm, native_hash) = vm_with_native(
        vec![double],
        2,
        Arc::new(|vm, args| {
            let value = vm.invoke(&args[0], vec![args[1].clone()])?;
            match value {
                Value::Number(n) => Ok(Value::Number(n + 1.0)),
                other => panic!("double returned {other:?}"),
            }
        }),
    );

    let outer = FunctionBuilder::new("test::outer")
        .push_value(Value::FunctionRef(double_hash))
        .push(5.0)
        .call_func(native_hash, 2)
        .push(100.0)
        .add()
        .build();
    let outer_hash = outer.hash;
    vm.load_function(outer);

    assert_eq!(vm.call(&outer_hash, Vec::new()), Ok(Value::Number(111.0)));
}

#[test]
fn test_nested_perform_is_delimited_at_the_invoke_boundary() {
    // A handler installed by the caller does not fire inside the invoke
    // region (its continuation would have to capture the native's Rust
    // frame): the nested perform falls through to the method's default
    // implementation instead.
    let default_impl = FunctionBuilder::new("test::default_seven")
        .push(7.0)
        .build();
    let method = test_method_ref(9, 1, Some(default_impl.hash));

    let performer = FunctionBuilder::new("test::performer")
        .suspend(&method, 0)
        .perform()
        .build();
    let performer_hash = performer.hash;

    // An arm that never resumes: if it fired, its 99 would replace the
    // whole delimited computation's value.
    let arm = FunctionBuilder::new("test::arm")
        .with_params(2)
        .with_locals(2)
        .push(99.0)
        .build();
    let arm_hash = arm.hash;

    let (mut vm, native_hash) = vm_with_native(
        vec![default_impl, performer, arm],
        1,
        Arc::new(|vm, args| vm.invoke(&args[0], Vec::new())),
    );

    let outer = FunctionBuilder::new("test::outer_handled")
        .with_builder(|b| {
            b.emit_make_handler(method.ability_id, &[(method.clone(), arm_hash)], 0);
            b.emit_handle_with_value();
        })
        .push_value(Value::FunctionRef(performer_hash))
        .call_func(native_hash, 1)
        .push(1.0)
        .add()
        .build();
    let outer_hash = outer.hash;
    vm.load_function(outer);

    // Default implementation (7) + 1, not the arm's 99.
    assert_eq!(vm.call(&outer_hash, Vec::new()), Ok(Value::Number(8.0)));
}

#[test]
fn test_handler_installed_inside_the_invoked_function_still_fires() {
    // The barrier hides only what is below the boundary: an abstract
    // method performed under a handler installed *within* the invoked
    // function dispatches normally.
    let method = test_method_ref(9, 2, None);

    let arm = FunctionBuilder::new("test::inner_arm")
        .with_params(2)
        .with_locals(2)
        .push(42.0)
        .build();
    let arm_hash = arm.hash;

    let handled = FunctionBuilder::new("test::handled_performer")
        .with_builder(|b| {
            b.emit_make_handler(method.ability_id, &[(method.clone(), arm_hash)], 0);
            b.emit_handle_with_value();
        })
        .suspend(&method, 0)
        .perform()
        .build();
    let handled_hash = handled.hash;

    let (mut vm, native_hash) = vm_with_native(
        vec![arm, handled],
        1,
        Arc::new(|vm, args| vm.invoke(&args[0], Vec::new())),
    );

    let outer = FunctionBuilder::new("test::outer_inner_handler")
        .push_value(Value::FunctionRef(handled_hash))
        .call_func(native_hash, 1)
        .build();
    let outer_hash = outer.hash;
    vm.load_function(outer);

    assert_eq!(vm.call(&outer_hash, Vec::new()), Ok(Value::Number(42.0)));
}

#[test]
fn test_failed_invoke_restores_the_caller_state() {
    // The invoked function faults (unhandled abstract method, no default);
    // the native absorbs the error and returns a fallback. The caller's
    // stack must be exactly as it was, so its arithmetic still lines up.
    let method = test_method_ref(11, 0, None);
    let aborter = FunctionBuilder::new("test::aborter")
        .suspend(&method, 0)
        .perform()
        .build();
    let aborter_hash = aborter.hash;

    let (mut vm, native_hash) = vm_with_native(
        vec![aborter],
        1,
        Arc::new(|vm, args| {
            Ok(vm
                .invoke(&args[0], Vec::new())
                .unwrap_or(Value::Number(-1.0)))
        }),
    );

    let outer = FunctionBuilder::new("test::outer_fallback")
        .push_value(Value::FunctionRef(aborter_hash))
        .call_func(native_hash, 1)
        .push(100.0)
        .add()
        .build();
    let outer_hash = outer.hash;
    vm.load_function(outer);

    assert_eq!(vm.call(&outer_hash, Vec::new()), Ok(Value::Number(99.0)));
}

#[test]
fn test_invoke_rejects_non_functions() {
    // `invoke` on a non-function value is a type error before anything is
    // pushed, and the caller continues cleanly if the native absorbs it.
    let (mut vm, native_hash) = vm_with_native(
        Vec::new(),
        1,
        Arc::new(|vm, args| {
            let error = vm
                .invoke(&args[0], Vec::new())
                .expect_err("a number is not invokable");
            assert!(matches!(error, crate::vm::VmError::TypeError { .. }));
            Ok(Value::Number(-1.0))
        }),
    );

    let outer = FunctionBuilder::new("test::outer_bad_callee")
        .push(5.0)
        .call_func(native_hash, 1)
        .push(100.0)
        .add()
        .build();
    let outer_hash = outer.hash;
    vm.load_function(outer);

    assert_eq!(vm.call(&outer_hash, Vec::new()), Ok(Value::Number(99.0)));
}

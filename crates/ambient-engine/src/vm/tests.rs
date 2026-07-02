//! VM unit tests.

use super::*;
use crate::bytecode::{BytecodeBuilder, Opcode};
use crate::test_utils::{Capture, FunctionBuilder, VmTest};
use crate::value::Value;
use std::sync::Arc;

// =========================================================================
// Constants and Stack Operations
// =========================================================================

#[test]
fn test_push_const_number() {
    VmTest::new().push(42.0).expect_number(42.0);
}

#[test]
fn test_push_const_bool() {
    VmTest::new().push_bool(true).expect_bool(true);
}

#[test]
fn test_push_const_string() {
    VmTest::new().push_str("hello").expect_string("hello");
}

#[test]
fn test_dup() {
    VmTest::new().push(21.0).dup().add().expect_number(42.0);
}

#[test]
fn test_pop() {
    VmTest::new()
        .push(1.0)
        .push(42.0)
        .push(2.0)
        .pop()
        .pop()
        .expect_number(1.0);
}

// =========================================================================
// Arithmetic Operations
// =========================================================================

#[test]
fn test_add() {
    VmTest::new()
        .push(10.0)
        .push(32.0)
        .add()
        .expect_number(42.0);
}

#[test]
fn test_sub() {
    VmTest::new().push(50.0).push(8.0).sub().expect_number(42.0);
}

#[test]
fn test_mul() {
    VmTest::new().push(6.0).push(7.0).mul().expect_number(42.0);
}

#[test]
fn test_div() {
    VmTest::new().push(84.0).push(2.0).div().expect_number(42.0);
}

#[test]
fn test_div_by_zero() {
    VmTest::new()
        .push(1.0)
        .push(0.0)
        .div()
        .expect_error(VmError::DivisionByZero);
}

#[test]
fn test_mod() {
    VmTest::new()
        .push(10.0)
        .push(3.0)
        .modulo()
        .expect_number(1.0);
}

#[test]
fn test_neg() {
    VmTest::new().push(42.0).neg().expect_number(-42.0);
}

// =========================================================================
// Comparison Operations
// =========================================================================

#[test]
fn test_eq_true() {
    VmTest::new().push(42.0).push(42.0).eq().expect_bool(true);
}

#[test]
fn test_eq_false() {
    VmTest::new().push(42.0).push(43.0).eq().expect_bool(false);
}

#[test]
fn test_lt() {
    VmTest::new().push(1.0).push(2.0).lt().expect_bool(true);
}

#[test]
fn test_le() {
    VmTest::new().push(2.0).push(2.0).le().expect_bool(true);
}

#[test]
fn test_gt() {
    VmTest::new().push(3.0).push(2.0).gt().expect_bool(true);
}

#[test]
fn test_ge() {
    VmTest::new().push(2.0).push(2.0).ge().expect_bool(true);
}

// =========================================================================
// Logic Operations
// =========================================================================

#[test]
fn test_and() {
    VmTest::new()
        .push_bool(true)
        .push_bool(false)
        .and()
        .expect_bool(false);
}

#[test]
fn test_or() {
    VmTest::new()
        .push_bool(true)
        .push_bool(false)
        .or()
        .expect_bool(true);
}

#[test]
fn test_not() {
    VmTest::new().push_bool(true).not().expect_bool(false);
}

// =========================================================================
// Type Errors
// =========================================================================

#[test]
fn test_type_error_add() {
    VmTest::new()
        .push(1.0)
        .push_bool(true)
        .add()
        .expect_error(VmError::TypeError {
            expected: "number",
            got: "bool",
            operation: "add",
        });
}

// =========================================================================
// Local Variables
// =========================================================================

#[test]
fn test_local_variables() {
    // x = 10; y = 32; x + y
    VmTest::new()
        .with_locals(2)
        .push(10.0)
        .store_local(0)
        .pop()
        .push(32.0)
        .store_local(1)
        .pop()
        .load_local(0)
        .load_local(1)
        .add()
        .expect_number(42.0);
}

// =========================================================================
// Control Flow
// =========================================================================

#[test]
fn test_jump() {
    VmTest::new()
        .with_builder(|b| {
            let jump_offset = b.emit_jump_placeholder(Opcode::Jump);
            b.emit_const(Value::Number(1.0)); // Skipped
            b.patch_jump(jump_offset);
            b.emit_const(Value::Number(42.0)); // Executed
        })
        .expect_number(42.0);
}

#[test]
fn test_jump_if_true() {
    VmTest::new()
        .with_builder(|b| {
            b.emit_const(Value::Bool(true));
            let jump_offset = b.emit_jump_placeholder(Opcode::JumpIf);
            b.emit_const(Value::Number(1.0));
            b.emit(Opcode::Return);
            b.patch_jump(jump_offset);
            b.emit_const(Value::Number(42.0));
        })
        .expect_number(42.0);
}

#[test]
fn test_jump_if_false() {
    VmTest::new()
        .with_builder(|b| {
            b.emit_const(Value::Bool(false));
            let jump_offset = b.emit_jump_placeholder(Opcode::JumpIf);
            b.emit_const(Value::Number(42.0));
            b.emit(Opcode::Return);
            b.patch_jump(jump_offset);
            b.emit_const(Value::Number(1.0));
        })
        .expect_number(42.0);
}

// =========================================================================
// Data Structures: Tuples
// =========================================================================

#[test]
fn test_make_tuple() {
    VmTest::new()
        .push(1.0)
        .push(2.0)
        .push(3.0)
        .make_tuple(3)
        .expect(Value::tuple(vec![
            Value::Number(1.0),
            Value::Number(2.0),
            Value::Number(3.0),
        ]));
}

#[test]
fn test_tuple_get() {
    VmTest::new()
        .push(1.0)
        .push(42.0)
        .push(3.0)
        .make_tuple(3)
        .tuple_get(1)
        .expect_number(42.0);
}

#[test]
fn test_tuple_index_out_of_bounds() {
    VmTest::new()
        .push(1.0)
        .make_tuple(1)
        .tuple_get(5)
        .expect_error(VmError::TupleIndexOutOfBounds {
            index: 5,
            length: 1,
        });
}

#[test]
fn test_tuple_unpacking() {
    // let pair = (10, 32); pair.0 + pair.1
    VmTest::new()
        .with_locals(1)
        .push(10.0)
        .push(32.0)
        .make_tuple(2)
        .store_local(0)
        .load_local(0)
        .tuple_get(0)
        .load_local(0)
        .tuple_get(1)
        .add()
        .expect_number(42.0);
}

// =========================================================================
// Data Structures: Records
// =========================================================================

#[test]
fn test_make_record() {
    VmTest::new()
        .push_str("x")
        .push(1.0)
        .push_str("y")
        .push(2.0)
        .make_record(2)
        .expect_record(|fields| {
            assert_eq!(fields.len(), 2);
            assert_eq!(fields.get(&Arc::from("x")), Some(&Value::Number(1.0)));
            assert_eq!(fields.get(&Arc::from("y")), Some(&Value::Number(2.0)));
        });
}

#[test]
fn test_record_get() {
    VmTest::new()
        .push_str("x")
        .push(42.0)
        .make_record(1)
        .record_get("x")
        .expect_number(42.0);
}

#[test]
fn test_record_manipulation_point() {
    // point = { x: 3.0, y: 4.0 }; point.x * point.x + point.y * point.y = 25.0
    VmTest::new()
        .with_locals(1)
        .push_str("x")
        .push(3.0)
        .push_str("y")
        .push(4.0)
        .make_record(2)
        .store_local(0)
        .load_local(0)
        .record_get("x")
        .load_local(0)
        .record_get("x")
        .mul()
        .load_local(0)
        .record_get("y")
        .load_local(0)
        .record_get("y")
        .mul()
        .add()
        .expect_number(25.0);
}

#[test]
fn test_record_nested_access() {
    // { user: { name: "Alice", age: 30 } }.user.age = 30.0
    VmTest::new()
        .with_locals(1)
        .push_str("name")
        .push_str("Alice")
        .push_str("age")
        .push(30.0)
        .make_record(2)
        .store_local(0)
        .pop()
        .push_str("user")
        .load_local(0)
        .make_record(1)
        .record_get("user")
        .record_get("age")
        .expect_number(30.0);
}

// =========================================================================
// Function Calls
// =========================================================================

#[test]
fn test_function_call() {
    let helper = FunctionBuilder::new("test::helper").push(42.0).build();
    let helper_hash = helper.hash;

    VmTest::new()
        .with_function(helper)
        .call_func(helper_hash, 0)
        .expect_number(42.0);
}

#[test]
fn test_function_with_args() {
    // add(a, b) = a + b
    let add_fn = FunctionBuilder::new("test::add")
        .with_locals(2)
        .with_params(2)
        .load_local(0)
        .load_local(1)
        .add()
        .build();
    let add_hash = add_fn.hash;

    VmTest::new()
        .with_function(add_fn)
        .push(10.0)
        .push(32.0)
        .call_func(add_hash, 2)
        .expect_number(42.0);
}

// =========================================================================
// Milestone 1: Recursive Functions
// =========================================================================

/// Build a recursive factorial function using FunctionBuilder.
fn build_factorial() -> crate::bytecode::CompiledFunction {
    FunctionBuilder::new("test::factorial")
        .with_locals(1)
        .with_params(1)
        .with_builder(|b| {
            let func_hash = blake3::hash(b"test::factorial");

            b.emit_u16(Opcode::LoadLocal, 0);
            b.emit_const(Value::Number(1.0));
            b.emit(Opcode::Le);

            let else_jump = b.emit_jump_placeholder(Opcode::JumpIfNot);
            b.emit_const(Value::Number(1.0));
            b.emit(Opcode::Return);

            b.patch_jump(else_jump);
            b.emit_u16(Opcode::LoadLocal, 0);
            b.emit_u16(Opcode::LoadLocal, 0);
            b.emit_const(Value::Number(1.0));
            b.emit(Opcode::Sub);
            b.emit_call(func_hash, 1);
            b.emit(Opcode::Mul);
        })
        .build()
}

#[test]
fn test_factorial_base_case() {
    let factorial = build_factorial();
    let hash = factorial.hash;

    VmTest::new()
        .with_function(factorial)
        .push(1.0)
        .call_func(hash, 1)
        .expect_number(1.0);
}

#[test]
fn test_factorial_small() {
    let factorial = build_factorial();
    let hash = factorial.hash;

    VmTest::new()
        .with_function(factorial)
        .push(5.0)
        .call_func(hash, 1)
        .expect_number(120.0);
}

#[test]
fn test_factorial_larger() {
    let factorial = build_factorial();
    let hash = factorial.hash;

    VmTest::new()
        .with_function(factorial)
        .push(10.0)
        .call_func(hash, 1)
        .expect_number(3_628_800.0);
}

/// Build a recursive fibonacci function using FunctionBuilder.
fn build_fibonacci() -> crate::bytecode::CompiledFunction {
    FunctionBuilder::new("test::fibonacci")
        .with_locals(1)
        .with_params(1)
        .with_builder(|b| {
            let func_hash = blake3::hash(b"test::fibonacci");

            b.emit_u16(Opcode::LoadLocal, 0);
            b.emit_const(Value::Number(1.0));
            b.emit(Opcode::Le);

            let else_jump = b.emit_jump_placeholder(Opcode::JumpIfNot);
            b.emit_u16(Opcode::LoadLocal, 0);
            b.emit(Opcode::Return);

            b.patch_jump(else_jump);

            // fib(n-1)
            b.emit_u16(Opcode::LoadLocal, 0);
            b.emit_const(Value::Number(1.0));
            b.emit(Opcode::Sub);
            b.emit_call(func_hash, 1);

            // fib(n-2)
            b.emit_u16(Opcode::LoadLocal, 0);
            b.emit_const(Value::Number(2.0));
            b.emit(Opcode::Sub);
            b.emit_call(func_hash, 1);

            b.emit(Opcode::Add);
        })
        .build()
}

#[test]
fn test_fibonacci_base_cases() {
    let fib = build_fibonacci();
    let hash = fib.hash;

    VmTest::new()
        .with_function(fib.clone())
        .push(0.0)
        .call_func(hash, 1)
        .expect_number(0.0);

    VmTest::new()
        .with_function(fib)
        .push(1.0)
        .call_func(hash, 1)
        .expect_number(1.0);
}

#[test]
fn test_fibonacci_sequence() {
    let fib = build_fibonacci();
    let hash = fib.hash;

    VmTest::new()
        .with_function(fib)
        .push(10.0)
        .call_func(hash, 1)
        .expect_number(55.0);
}

#[test]
fn test_fibonacci_values() {
    let expected = [0.0, 1.0, 1.0, 2.0, 3.0, 5.0, 8.0, 13.0, 21.0, 34.0, 55.0];

    for (n, exp) in expected.iter().enumerate() {
        let fib = build_fibonacci();
        let hash = fib.hash;

        let result = VmTest::new()
            .with_function(fib)
            .push(n as f64)
            .call_func(hash, 1)
            .run();

        assert_eq!(result, Ok(Value::Number(*exp)), "fib({n}) should be {exp}");
    }
}

// =========================================================================
// Milestone 2: Abilities and Handlers
// =========================================================================

/// Distinct, recognizable synthetic AbilityIds for tests.
const ABILITY_CONSOLE: crate::types::AbilityId = crate::types::AbilityId::from_bytes([1; 32]);
const ABILITY_MATH: crate::types::AbilityId = crate::types::AbilityId::from_bytes([2; 32]);
const METHOD_PRINT: u16 = 0;
const METHOD_DOUBLE: u16 = 0;
const METHOD_ADD_TEN: u16 = 1;

#[test]
fn test_suspend_creates_ability_value() {
    VmTest::new()
        .push(42.0)
        .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
        .expect_suspended(|ability| {
            assert_eq!(ability.ability_id, ABILITY_CONSOLE);
            assert_eq!(ability.method_id, METHOD_PRINT);
            assert_eq!(ability.args.len(), 1);
            assert_eq!(ability.args[0], Value::Number(42.0));
        });
}

#[test]
fn test_host_handler_called() {
    let capture = Capture::<f64>::new();
    let log = capture.clone_inner();

    VmTest::new()
        .push(42.0)
        .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
        .perform()
        .with_host_handler(ABILITY_CONSOLE, METHOD_PRINT, move |ability| {
            if let Value::Number(n) = &ability.args[0] {
                log.lock().expect("lock").push(*n);
            }
            Ok(Value::Unit)
        })
        .expect_unit();

    capture.assert_eq(&[42.0]);
}

#[test]
fn test_host_handler_returns_value() {
    VmTest::new()
        .push(21.0)
        .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
        .perform()
        .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
            if let Value::Number(n) = &ability.args[0] {
                Ok(Value::Number(n * 2.0))
            } else {
                Ok(Value::Unit)
            }
        })
        .expect_number(42.0);
}

#[test]
fn test_bytecode_handler_overrides_host_handler() {
    // Host handler would return 999.0, but bytecode handler should win with 42.0
    let handler = FunctionBuilder::new("test::override_handler")
        .with_locals(2)
        .with_params(2)
        .load_local(0) // continuation
        .push(42.0) // resume value
        .resume()
        .build();
    let handler_hash = handler.hash;

    VmTest::new()
        .with_function(handler)
        .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |_ability| {
            // This should NOT be called - bytecode handler takes priority
            Ok(Value::Number(999.0))
        })
        .handle(ABILITY_MATH, handler_hash)
        .push(5.0)
        .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
        .perform()
        .unhandle()
        .expect_number(42.0); // Bytecode handler wins, not host handler's 999.0
}

#[test]
fn test_unhandled_ability_error() {
    VmTest::new()
        .push(42.0)
        .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
        .perform()
        .expect_error(VmError::UnhandledAbility {
            ability_id: ABILITY_CONSOLE,
            method_id: METHOD_PRINT,
        });
}

#[test]
fn test_bytecode_handler_simple_resume() {
    // Handler: receives (continuation, ability), resumes with 42.0
    let handler = FunctionBuilder::new("test::math_handler")
        .with_locals(2)
        .with_params(2)
        .load_local(0)
        .push(42.0)
        .resume()
        .build();
    let handler_hash = handler.hash;

    VmTest::new()
        .with_function(handler)
        .handle(ABILITY_MATH, handler_hash)
        .push(5.0)
        .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
        .perform()
        .unhandle()
        .expect_number(42.0);
}

#[test]
fn test_single_shot_enforcement() {
    // Handler resumes once and returns
    let handler = FunctionBuilder::new("test::double_resume_handler")
        .with_locals(2)
        .with_params(2)
        .load_local(0)
        .push(1.0)
        .resume()
        .build();
    let handler_hash = handler.hash;

    VmTest::new()
        .with_function(handler)
        .handle(ABILITY_MATH, handler_hash)
        .push(5.0)
        .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
        .perform()
        .unhandle()
        .expect_number(1.0);
}

#[test]
fn test_perform_expected_type_error() {
    VmTest::new()
        .push(42.0)
        .perform()
        .expect_error(VmError::ExpectedSuspendedAbility { got: "number" });
}

#[test]
fn test_multiple_ability_calls() {
    let capture = Capture::<f64>::new();
    let log = capture.clone_inner();

    VmTest::new()
        .push(1.0)
        .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
        .perform()
        .pop()
        .push(2.0)
        .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
        .perform()
        .pop()
        .push(3.0)
        .suspend(ABILITY_CONSOLE, METHOD_PRINT, 1)
        .perform()
        .with_host_handler(ABILITY_CONSOLE, METHOD_PRINT, move |ability| {
            if let Value::Number(n) = &ability.args[0] {
                log.lock().expect("lock").push(*n);
            }
            Ok(Value::Unit)
        })
        .expect_unit();

    capture.assert_eq(&[1.0, 2.0, 3.0]);
}

#[test]
fn test_ability_with_multiple_args() {
    VmTest::new()
        .push(10.0)
        .push(32.0)
        .suspend(ABILITY_MATH, METHOD_ADD_TEN, 2)
        .perform()
        .with_host_handler(ABILITY_MATH, METHOD_ADD_TEN, |ability| {
            if ability.args.len() >= 2 {
                if let (Value::Number(a), Value::Number(b)) = (&ability.args[0], &ability.args[1]) {
                    return Ok(Value::Number(a + b));
                }
            }
            Ok(Value::Unit)
        })
        .expect_number(42.0);
}

// =========================================================================
// Milestone 3: Abilities as Values
// =========================================================================

#[test]
fn test_ability_stored_in_variable() {
    let capture = Capture::<u32>::new();
    let count = capture.clone_inner();

    VmTest::new()
        .with_locals(1)
        .push(21.0)
        .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
        .store_local(0)
        .pop()
        .push(999.0)
        .pop()
        .load_local(0)
        .perform()
        .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, move |ability| {
            count.lock().expect("lock").push(1);
            if let Value::Number(n) = &ability.args[0] {
                Ok(Value::Number(n * 2.0))
            } else {
                Ok(Value::Unit)
            }
        })
        .expect_number(42.0);

    capture.assert_eq(&[1]);
}

#[test]
fn test_ability_stored_in_tuple() {
    VmTest::new()
        .with_locals(1)
        .push(21.0)
        .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
        .push_str("label")
        .make_tuple(2)
        .store_local(0)
        .pop()
        .load_local(0)
        .tuple_get(0)
        .perform()
        .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
            if let Value::Number(n) = &ability.args[0] {
                Ok(Value::Number(n * 2.0))
            } else {
                Ok(Value::Unit)
            }
        })
        .expect_number(42.0);
}

#[test]
fn test_ability_passed_to_function() {
    // perform_ability(op) = op!
    let perform_fn = FunctionBuilder::new("test::perform_ability")
        .with_locals(1)
        .with_params(1)
        .load_local(0)
        .perform()
        .build();
    let perform_hash = perform_fn.hash;

    VmTest::new()
        .with_function(perform_fn)
        .push(21.0)
        .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
        .call_func(perform_hash, 1)
        .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
            if let Value::Number(n) = &ability.args[0] {
                Ok(Value::Number(n * 2.0))
            } else {
                Ok(Value::Unit)
            }
        })
        .expect_number(42.0);
}

#[test]
fn test_multiple_abilities_different_order() {
    // op1 = double(10), op2 = double(21), perform op2
    VmTest::new()
        .with_locals(2)
        .push(10.0)
        .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
        .store_local(0)
        .pop()
        .push(21.0)
        .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
        .store_local(1)
        .pop()
        .load_local(1)
        .perform()
        .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
            if let Value::Number(n) = &ability.args[0] {
                Ok(Value::Number(n * 2.0))
            } else {
                Ok(Value::Unit)
            }
        })
        .expect_number(42.0);
}

#[test]
fn test_ability_equality() {
    VmTest::new()
        .with_locals(2)
        .push(42.0)
        .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
        .store_local(0)
        .pop()
        .push(42.0)
        .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
        .store_local(1)
        .pop()
        .load_local(0)
        .load_local(1)
        .eq()
        .expect_bool(true);
}

#[test]
fn test_ability_returned_from_function() {
    // create_double_op(n) = Math.double(n) (no perform)
    let creator_fn = FunctionBuilder::new("test::create_double_op")
        .with_locals(1)
        .with_params(1)
        .load_local(0)
        .suspend(ABILITY_MATH, METHOD_DOUBLE, 1)
        .build();
    let creator_hash = creator_fn.hash;

    VmTest::new()
        .with_locals(1)
        .with_function(creator_fn)
        .push(21.0)
        .call_func(creator_hash, 1)
        .store_local(0)
        .pop()
        .load_local(0)
        .perform()
        .with_host_handler(ABILITY_MATH, METHOD_DOUBLE, |ability| {
            if let Value::Number(n) = &ability.args[0] {
                Ok(Value::Number(n * 2.0))
            } else {
                Ok(Value::Unit)
            }
        })
        .expect_number(42.0);
}

#[test]
fn test_make_handler_creates_handler_value() {
    use crate::abilities::console;

    // Create a simple handler method function that returns unit.
    let mut handler_builder = BytecodeBuilder::new();
    handler_builder.emit_const(Value::Unit);
    handler_builder.emit(Opcode::Return);
    let handler_func = handler_builder.build(2, 2);
    let handler_hash = handler_func.hash;

    // Create main function that makes a handler and returns it.
    let mut builder = BytecodeBuilder::new();

    // Emit MakeHandler: Console ability, 1 method (print), 0 captures.
    builder.emit_make_handler(
        console::ability_id(),
        &[(console::METHOD_PRINT, handler_hash)],
        0,
    );

    // Return the handler value.
    builder.emit(Opcode::Return);

    let main_func = builder.build(0, 0);
    let main_hash = main_func.hash;

    let mut vm = Vm::new();
    vm.load_function(handler_func);
    vm.load_function(main_func);

    let result = vm.call(&main_hash, vec![]);

    // Should return a handler value.
    assert!(result.is_ok(), "Should succeed: {:?}", result);
    if let Ok(Value::Handler(handler)) = result {
        assert_eq!(handler.ability_id, console::ability_id());
        assert!(handler.handles_method(console::METHOD_PRINT));
        assert_eq!(handler.methods.len(), 1);
    } else {
        panic!("Expected Handler value, got {:?}", result);
    }
}

#[test]
fn test_make_handler_with_multiple_methods() {
    use crate::abilities::console;

    // Create handler method functions.
    let mut print_builder = BytecodeBuilder::new();
    print_builder.emit_const(Value::Unit);
    print_builder.emit(Opcode::Return);
    let print_func = print_builder.build(2, 2);
    let print_hash = print_func.hash;

    let mut eprint_builder = BytecodeBuilder::new();
    eprint_builder.emit_const(Value::Unit);
    eprint_builder.emit(Opcode::Return);
    let eprint_func = eprint_builder.build(2, 2);
    let eprint_hash = eprint_func.hash;

    // Create main function that makes a handler with 2 methods.
    let mut builder = BytecodeBuilder::new();
    builder.emit_make_handler(
        console::ability_id(),
        &[
            (console::METHOD_PRINT, print_hash),
            (console::METHOD_EPRINT, eprint_hash),
        ],
        0,
    );
    builder.emit(Opcode::Return);

    let main_func = builder.build(0, 0);
    let main_hash = main_func.hash;

    let mut vm = Vm::new();
    vm.load_function(print_func);
    vm.load_function(eprint_func);
    vm.load_function(main_func);

    let result = vm.call(&main_hash, vec![]);

    assert!(result.is_ok(), "Should succeed: {:?}", result);
    if let Ok(Value::Handler(handler)) = result {
        assert_eq!(handler.ability_id, console::ability_id());
        assert!(handler.handles_method(console::METHOD_PRINT));
        assert!(handler.handles_method(console::METHOD_EPRINT));
        assert_eq!(handler.methods.len(), 2);
    } else {
        panic!("Expected Handler value, got {:?}", result);
    }
}

// =========================================================================
// List Operations (Milestone 15)
// =========================================================================

#[test]
fn test_make_list() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(1.0));
    builder.emit_const(Value::Number(2.0));
    builder.emit_const(Value::Number(3.0));
    builder.emit_make_list(3);
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert!(result.is_ok());
    if let Ok(Value::List(elements)) = result {
        assert_eq!(elements.len(), 3);
        assert_eq!(elements[0], Value::Number(1.0));
        assert_eq!(elements[1], Value::Number(2.0));
        assert_eq!(elements[2], Value::Number(3.0));
    } else {
        panic!("Expected List, got {:?}", result);
    }
}

#[test]
fn test_list_get() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(10.0));
    builder.emit_const(Value::Number(20.0));
    builder.emit_const(Value::Number(30.0));
    builder.emit_make_list(3);
    builder.emit_const(Value::Number(1.0)); // index
    builder.emit_list_get();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Number(20.0)));
}

#[test]
fn test_list_length() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(1.0));
    builder.emit_const(Value::Number(2.0));
    builder.emit_const(Value::Number(3.0));
    builder.emit_const(Value::Number(4.0));
    builder.emit_make_list(4);
    builder.emit_list_length();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Number(4.0)));
}

#[test]
fn test_list_concat() {
    let mut builder = BytecodeBuilder::new();
    // First list [1, 2]
    builder.emit_const(Value::Number(1.0));
    builder.emit_const(Value::Number(2.0));
    builder.emit_make_list(2);
    // Second list [3, 4]
    builder.emit_const(Value::Number(3.0));
    builder.emit_const(Value::Number(4.0));
    builder.emit_make_list(2);
    // Concat
    builder.emit_list_concat();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert!(result.is_ok());
    if let Ok(Value::List(elements)) = result {
        assert_eq!(elements.len(), 4);
        assert_eq!(elements[0], Value::Number(1.0));
        assert_eq!(elements[1], Value::Number(2.0));
        assert_eq!(elements[2], Value::Number(3.0));
        assert_eq!(elements[3], Value::Number(4.0));
    } else {
        panic!("Expected List, got {:?}", result);
    }
}

#[test]
fn test_list_append() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(1.0));
    builder.emit_const(Value::Number(2.0));
    builder.emit_make_list(2);
    builder.emit_const(Value::Number(3.0));
    builder.emit_list_append();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert!(result.is_ok());
    if let Ok(Value::List(elements)) = result {
        assert_eq!(elements.len(), 3);
        assert_eq!(elements[2], Value::Number(3.0));
    } else {
        panic!("Expected List, got {:?}", result);
    }
}

#[test]
fn test_list_head() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(1.0));
    builder.emit_const(Value::Number(2.0));
    builder.emit_const(Value::Number(3.0));
    builder.emit_make_list(3);
    builder.emit_list_head();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Number(1.0)));
}

#[test]
fn test_list_tail() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(1.0));
    builder.emit_const(Value::Number(2.0));
    builder.emit_const(Value::Number(3.0));
    builder.emit_make_list(3);
    builder.emit_list_tail();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert!(result.is_ok());
    if let Ok(Value::List(elements)) = result {
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0], Value::Number(2.0));
        assert_eq!(elements[1], Value::Number(3.0));
    } else {
        panic!("Expected List, got {:?}", result);
    }
}

// =========================================================================
// String Operations (Milestone 15)
// =========================================================================

#[test]
fn test_string_length() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("hello"));
    builder.emit_string_length();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Number(5.0)));
}

#[test]
fn test_string_split() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("a,b,c"));
    builder.emit_const(Value::string(","));
    builder.emit_string_split();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert!(result.is_ok());
    if let Ok(Value::List(elements)) = result {
        assert_eq!(elements.len(), 3);
        assert_eq!(elements[0], Value::string("a"));
        assert_eq!(elements[1], Value::string("b"));
        assert_eq!(elements[2], Value::string("c"));
    } else {
        panic!("Expected List, got {:?}", result);
    }
}

#[test]
fn test_string_join() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("a"));
    builder.emit_const(Value::string("b"));
    builder.emit_const(Value::string("c"));
    builder.emit_make_list(3);
    builder.emit_const(Value::string("-"));
    builder.emit_string_join();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::string("a-b-c")));
}

#[test]
fn test_string_trim() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("  hello world  "));
    builder.emit_string_trim();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::string("hello world")));
}

#[test]
fn test_string_contains() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("hello world"));
    builder.emit_const(Value::string("world"));
    builder.emit_string_contains();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Bool(true)));
}

#[test]
fn test_string_contains_false() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("hello world"));
    builder.emit_const(Value::string("foo"));
    builder.emit_string_contains();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Bool(false)));
}

#[test]
fn test_string_concat() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("hello"));
    builder.emit_const(Value::string(" world"));
    builder.emit_string_concat();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::string("hello world")));
}

// =========================================================================
// Type Conversion (Milestone 15)
// =========================================================================

#[test]
fn test_to_string_number() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(42.0));
    builder.emit_to_string();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::string("42")));
}

#[test]
fn test_to_string_bool() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Bool(true));
    builder.emit_to_string();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::string("true")));
}

#[test]
fn test_to_string_list() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(1.0));
    builder.emit_const(Value::Number(2.0));
    builder.emit_make_list(2);
    builder.emit_to_string();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::string("[1, 2]")));
}

#[test]
fn test_parse_number_success() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("42.5"));
    builder.emit_parse_number();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert!(result.is_ok());
    if let Ok(Value::Tuple(elements)) = result {
        assert_eq!(elements[0], Value::Bool(true));
        assert_eq!(elements[1], Value::Number(42.5));
    } else {
        panic!("Expected Tuple, got {:?}", result);
    }
}

#[test]
fn test_parse_number_failure() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("not a number"));
    builder.emit_parse_number();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert!(result.is_ok());
    if let Ok(Value::Tuple(elements)) = result {
        assert_eq!(elements[0], Value::Bool(false));
    } else {
        panic!("Expected Tuple, got {:?}", result);
    }
}

#[test]
fn test_parse_bool_success() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("true"));
    builder.emit_parse_bool();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert!(result.is_ok());
    if let Ok(Value::Tuple(elements)) = result {
        assert_eq!(elements[0], Value::Bool(true));
        assert_eq!(elements[1], Value::Bool(true));
    } else {
        panic!("Expected Tuple, got {:?}", result);
    }
}

#[test]
fn test_parse_bool_failure() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("maybe"));
    builder.emit_parse_bool();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert!(result.is_ok());
    if let Ok(Value::Tuple(elements)) = result {
        assert_eq!(elements[0], Value::Bool(false));
    } else {
        panic!("Expected Tuple, got {:?}", result);
    }
}

// =========================================================================
// Set Operations (Milestone 15)
// =========================================================================

#[test]
fn test_set_empty() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_make_empty_set();
    builder.emit_set_length();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Number(0.0)));
}

#[test]
fn test_set_from_values() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(1.0));
    builder.emit_const(Value::Number(2.0));
    builder.emit_const(Value::Number(3.0));
    builder.emit_make_set(3);
    builder.emit_set_length();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Number(3.0)));
}

#[test]
fn test_set_insert() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_make_empty_set();
    builder.emit_const(Value::Number(42.0));
    builder.emit_set_insert();
    builder.emit_set_length();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Number(1.0)));
}

#[test]
fn test_set_insert_duplicate() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_make_empty_set();
    builder.emit_const(Value::Number(42.0));
    builder.emit_set_insert();
    builder.emit_const(Value::Number(42.0)); // Same value
    builder.emit_set_insert();
    builder.emit_set_length();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    // Inserting a duplicate should not increase size
    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Number(1.0)));
}

#[test]
fn test_set_contains() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(1.0));
    builder.emit_const(Value::Number(2.0));
    builder.emit_const(Value::Number(3.0));
    builder.emit_make_set(3);
    builder.emit_const(Value::Number(2.0));
    builder.emit_set_contains();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Bool(true)));
}

#[test]
fn test_set_contains_missing() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(1.0));
    builder.emit_const(Value::Number(2.0));
    builder.emit_make_set(2);
    builder.emit_const(Value::Number(99.0));
    builder.emit_set_contains();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Bool(false)));
}

#[test]
fn test_set_remove() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(1.0));
    builder.emit_const(Value::Number(2.0));
    builder.emit_const(Value::Number(3.0));
    builder.emit_make_set(3);
    builder.emit_const(Value::Number(2.0));
    builder.emit_set_remove();
    builder.emit_set_length();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Number(2.0)));
}

#[test]
fn test_set_union() {
    let mut builder = BytecodeBuilder::new();
    // Set 1: {1, 2}
    builder.emit_const(Value::Number(1.0));
    builder.emit_const(Value::Number(2.0));
    builder.emit_make_set(2);
    // Set 2: {2, 3}
    builder.emit_const(Value::Number(2.0));
    builder.emit_const(Value::Number(3.0));
    builder.emit_make_set(2);
    // Union
    builder.emit_set_union();
    builder.emit_set_length();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    // Union of {1, 2} and {2, 3} = {1, 2, 3}
    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Number(3.0)));
}

#[test]
fn test_set_intersection() {
    let mut builder = BytecodeBuilder::new();
    // Set 1: {1, 2, 3}
    builder.emit_const(Value::Number(1.0));
    builder.emit_const(Value::Number(2.0));
    builder.emit_const(Value::Number(3.0));
    builder.emit_make_set(3);
    // Set 2: {2, 3, 4}
    builder.emit_const(Value::Number(2.0));
    builder.emit_const(Value::Number(3.0));
    builder.emit_const(Value::Number(4.0));
    builder.emit_make_set(3);
    // Intersection
    builder.emit_set_intersection();
    builder.emit_set_length();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    // Intersection of {1, 2, 3} and {2, 3, 4} = {2, 3}
    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Number(2.0)));
}

#[test]
fn test_set_difference() {
    let mut builder = BytecodeBuilder::new();
    // Set 1: {1, 2, 3}
    builder.emit_const(Value::Number(1.0));
    builder.emit_const(Value::Number(2.0));
    builder.emit_const(Value::Number(3.0));
    builder.emit_make_set(3);
    // Set 2: {2}
    builder.emit_const(Value::Number(2.0));
    builder.emit_make_set(1);
    // Difference
    builder.emit_set_difference();
    builder.emit_set_length();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    // Difference of {1, 2, 3} - {2} = {1, 3}
    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Number(2.0)));
}

#[test]
fn test_set_to_list() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(1.0));
    builder.emit_const(Value::Number(2.0));
    builder.emit_make_set(2);
    builder.emit_set_to_list();
    builder.emit_list_length();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Number(2.0)));
}

#[test]
fn test_set_with_strings() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("a"));
    builder.emit_const(Value::string("b"));
    builder.emit_const(Value::string("a")); // Duplicate
    builder.emit_make_set(3);
    builder.emit_set_length();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    // Should only have 2 unique values
    let result = vm.call(&hash, vec![]);
    assert_eq!(result, Ok(Value::Number(2.0)));
}

// =========================================================================
// Enum Operations
// =========================================================================

#[test]
fn test_make_enum_none() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_none(); // Option::None
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::none());
}

#[test]
fn test_make_enum_some() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(42.0));
    builder.emit_some(); // Option::Some(42.0)
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::some(Value::Number(42.0)));
}

#[test]
fn test_make_enum_result_ok() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("success"));
    builder.emit_ok(); // Result::Ok("success")
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::ok(Value::string("success")));
}

#[test]
fn test_make_enum_result_err() {
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("error"));
    builder.emit_err(); // Result::Err("error")
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::err(Value::string("error")));
}

#[test]
fn test_enum_is() {
    // Check if Option::Some(42) is tag 1 (Some)
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(42.0));
    builder.emit_some();
    builder.emit_enum_is(1); // Check if it's Some (tag 1)
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_enum_is_false() {
    // Check if Option::None is tag 1 (Some) - should be false
    let mut builder = BytecodeBuilder::new();
    builder.emit_none();
    builder.emit_enum_is(1); // Check if it's Some (tag 1)
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::Bool(false));
}

#[test]
fn test_enum_payload() {
    // Extract payload from Option::Some(42)
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(42.0));
    builder.emit_some();
    builder.emit_enum_payload();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::Number(42.0));
}

#[test]
fn test_enum_payload_missing() {
    // Try to extract payload from Option::None - should error
    let mut builder = BytecodeBuilder::new();
    builder.emit_none();
    builder.emit_enum_payload();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]);
    assert!(matches!(result, Err(VmError::EnumPayloadMissing { .. })));
}

#[test]
fn test_enum_tag() {
    // Get tag from Option::Some(42) - should be 1
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(42.0));
    builder.emit_some();
    builder.emit_enum_tag();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::Number(1.0)); // Some is tag 1
}

#[test]
fn test_enum_tag_none() {
    // Get tag from Option::None - should be 0
    let mut builder = BytecodeBuilder::new();
    builder.emit_none();
    builder.emit_enum_tag();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::Number(0.0)); // None is tag 0
}

// ─────────────────────────────────────────────────────────────────────────
// Option/Result utility tests
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_option_unwrap_or_some() {
    // Some(42).unwrap_or(0) should return 42
    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(42.0));
    builder.emit_some();
    builder.emit_const(Value::Number(0.0)); // default
    builder.emit_option_unwrap_or();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::Number(42.0));
}

#[test]
fn test_option_unwrap_or_none() {
    // None.unwrap_or(99) should return 99
    let mut builder = BytecodeBuilder::new();
    builder.emit_none();
    builder.emit_const(Value::Number(99.0)); // default
    builder.emit_option_unwrap_or();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::Number(99.0));
}

#[test]
fn test_option_unwrap_or_string() {
    // None.unwrap_or("default") should return "default"
    let mut builder = BytecodeBuilder::new();
    builder.emit_none();
    builder.emit_const(Value::string("default"));
    builder.emit_option_unwrap_or();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::string("default"));
}

// =========================================================================
// Option/Result Map Operations (Milestone 15 - Closure-based utilities)
// =========================================================================

/// Helper to create a simple closure that adds a constant to its argument.
fn make_add_closure(add_value: f64) -> (crate::bytecode::CompiledFunction, blake3::Hash) {
    // closure(x) => x + add_value
    let mut builder = BytecodeBuilder::new();
    builder.emit_u16(Opcode::LoadLocal, 0); // x
    builder.emit_const(Value::Number(add_value));
    builder.emit(Opcode::Add);
    builder.emit(Opcode::Return);
    let func = builder.build(1, 1);
    let hash = func.hash;
    (func, hash)
}

/// Helper to create a closure that doubles its argument.
fn make_double_closure() -> (crate::bytecode::CompiledFunction, blake3::Hash) {
    let mut builder = BytecodeBuilder::new();
    builder.emit_u16(Opcode::LoadLocal, 0);
    builder.emit_const(Value::Number(2.0));
    builder.emit(Opcode::Mul);
    builder.emit(Opcode::Return);
    let func = builder.build(1, 1);
    let hash = func.hash;
    (func, hash)
}

/// Helper to create a closure that wraps its argument in Some.
fn make_some_closure() -> (crate::bytecode::CompiledFunction, blake3::Hash) {
    let mut builder = BytecodeBuilder::new();
    builder.emit_u16(Opcode::LoadLocal, 0);
    builder.emit_some();
    builder.emit(Opcode::Return);
    let func = builder.build(1, 1);
    let hash = func.hash;
    (func, hash)
}

/// Helper to create a closure that returns None.
fn make_none_closure() -> (crate::bytecode::CompiledFunction, blake3::Hash) {
    let mut builder = BytecodeBuilder::new();
    builder.emit_none();
    builder.emit(Opcode::Return);
    let func = builder.build(1, 1);
    let hash = func.hash;
    (func, hash)
}

/// Helper to create a closure that wraps its argument in Ok.
fn make_ok_closure() -> (crate::bytecode::CompiledFunction, blake3::Hash) {
    let mut builder = BytecodeBuilder::new();
    builder.emit_u16(Opcode::LoadLocal, 0);
    builder.emit_ok();
    builder.emit(Opcode::Return);
    let func = builder.build(1, 1);
    let hash = func.hash;
    (func, hash)
}

#[test]
fn test_option_map_some() {
    // Some(10).map(|x| x + 32) should return Some(42)
    let (closure_func, closure_hash) = make_add_closure(32.0);

    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(10.0));
    builder.emit_some(); // Some(10)
    builder.emit_make_closure(closure_hash, 0); // closure with no captures
    builder.emit_option_map();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(closure_func);
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::some(Value::Number(42.0)));
}

#[test]
fn test_option_map_none() {
    // None.map(|x| x + 32) should return None
    let (closure_func, closure_hash) = make_add_closure(32.0);

    let mut builder = BytecodeBuilder::new();
    builder.emit_none();
    builder.emit_make_closure(closure_hash, 0);
    builder.emit_option_map();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(closure_func);
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::none());
}

#[test]
fn test_option_map_double() {
    // Some(21).map(|x| x * 2) should return Some(42)
    let (closure_func, closure_hash) = make_double_closure();

    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(21.0));
    builder.emit_some();
    builder.emit_make_closure(closure_hash, 0);
    builder.emit_option_map();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(closure_func);
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::some(Value::Number(42.0)));
}

#[test]
fn test_option_and_then_some_to_some() {
    // Some(42).and_then(|x| Some(x)) should return Some(42)
    let (closure_func, closure_hash) = make_some_closure();

    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(42.0));
    builder.emit_some();
    builder.emit_make_closure(closure_hash, 0);
    builder.emit_option_and_then();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(closure_func);
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::some(Value::Number(42.0)));
}

#[test]
fn test_option_and_then_some_to_none() {
    // Some(42).and_then(|_| None) should return None
    let (closure_func, closure_hash) = make_none_closure();

    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(42.0));
    builder.emit_some();
    builder.emit_make_closure(closure_hash, 0);
    builder.emit_option_and_then();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(closure_func);
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::none());
}

#[test]
fn test_option_and_then_none() {
    // None.and_then(|x| Some(x)) should return None
    let (closure_func, closure_hash) = make_some_closure();

    let mut builder = BytecodeBuilder::new();
    builder.emit_none();
    builder.emit_make_closure(closure_hash, 0);
    builder.emit_option_and_then();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(closure_func);
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::none());
}

#[test]
fn test_result_map_ok() {
    // Ok(10).map(|x| x + 32) should return Ok(42)
    let (closure_func, closure_hash) = make_add_closure(32.0);

    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(10.0));
    builder.emit_ok(); // Ok(10)
    builder.emit_make_closure(closure_hash, 0);
    builder.emit_result_map();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(closure_func);
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::ok(Value::Number(42.0)));
}

#[test]
fn test_result_map_err_unchanged() {
    // Err("error").map(|x| x + 32) should return Err("error")
    let (closure_func, closure_hash) = make_add_closure(32.0);

    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("error"));
    builder.emit_err(); // Err("error")
    builder.emit_make_closure(closure_hash, 0);
    builder.emit_result_map();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(closure_func);
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::err(Value::string("error")));
}

#[test]
fn test_result_map_err_transforms() {
    // Err("error").map_err(|_| "new_error") should return Err("new_error")
    // Create a closure that ignores input and returns "new_error"
    let mut closure_builder = BytecodeBuilder::new();
    closure_builder.emit_const(Value::string("new_error"));
    closure_builder.emit(Opcode::Return);
    let closure_func = closure_builder.build(1, 1);
    let closure_hash = closure_func.hash;

    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("error"));
    builder.emit_err();
    builder.emit_make_closure(closure_hash, 0);
    builder.emit_result_map_err();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(closure_func);
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::err(Value::string("new_error")));
}

#[test]
fn test_result_map_err_ok_unchanged() {
    // Ok(42).map_err(|_| "new_error") should return Ok(42)
    let mut closure_builder = BytecodeBuilder::new();
    closure_builder.emit_const(Value::string("new_error"));
    closure_builder.emit(Opcode::Return);
    let closure_func = closure_builder.build(1, 1);
    let closure_hash = closure_func.hash;

    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(42.0));
    builder.emit_ok();
    builder.emit_make_closure(closure_hash, 0);
    builder.emit_result_map_err();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(closure_func);
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::ok(Value::Number(42.0)));
}

#[test]
fn test_result_and_then_ok_to_ok() {
    // Ok(42).and_then(|x| Ok(x)) should return Ok(42)
    let (closure_func, closure_hash) = make_ok_closure();

    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(42.0));
    builder.emit_ok();
    builder.emit_make_closure(closure_hash, 0);
    builder.emit_result_and_then();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(closure_func);
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::ok(Value::Number(42.0)));
}

#[test]
fn test_result_and_then_ok_to_err() {
    // Ok(42).and_then(|_| Err("oops")) should return Err("oops")

    // Build a closure that returns a constant error
    let mut closure_builder = BytecodeBuilder::new();
    closure_builder.emit_const(Value::string("oops"));
    closure_builder.emit_err();
    closure_builder.emit(Opcode::Return);
    let closure_func = closure_builder.build(1, 1);
    let closure_hash = closure_func.hash;

    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(42.0));
    builder.emit_ok();
    builder.emit_make_closure(closure_hash, 0);
    builder.emit_result_and_then();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(closure_func);
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::err(Value::string("oops")));
}

#[test]
fn test_result_and_then_err_unchanged() {
    // Err("error").and_then(|x| Ok(x)) should return Err("error")
    let (closure_func, closure_hash) = make_ok_closure();

    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::string("error"));
    builder.emit_err();
    builder.emit_make_closure(closure_hash, 0);
    builder.emit_result_and_then();
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(closure_func);
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::err(Value::string("error")));
}

#[test]
fn test_option_map_chained() {
    // Some(10).map(|x| x + 10).map(|x| x * 2) should return Some(40)
    let (add_func, add_hash) = make_add_closure(10.0);
    let (double_func, double_hash) = make_double_closure();

    let mut builder = BytecodeBuilder::new();
    builder.emit_const(Value::Number(10.0));
    builder.emit_some(); // Some(10)
    builder.emit_make_closure(add_hash, 0);
    builder.emit_option_map(); // Some(20)
    builder.emit_make_closure(double_hash, 0);
    builder.emit_option_map(); // Some(40)
    builder.emit(Opcode::Return);

    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(add_func);
    vm.load_function(double_func);
    vm.load_function(func);

    let result = vm.call(&hash, vec![]).unwrap();
    assert_eq!(result, Value::some(Value::Number(40.0)));
}

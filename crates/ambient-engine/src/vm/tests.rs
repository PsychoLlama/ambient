//! VM unit tests.

use super::*;
use crate::bytecode::{BytecodeBuilder, Opcode};
use crate::test_utils::{FunctionBuilder, VmTest};
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
fn test_load_object_pushes_stored_value() {
    let object_hash = blake3::hash(b"const-answer");
    let mut builder = BytecodeBuilder::new();
    builder.emit_load_object(object_hash);
    builder.emit(Opcode::Return);
    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);
    vm.load_value(object_hash, Value::Number(42.0));

    assert_eq!(vm.call(&hash, vec![]), Ok(Value::Number(42.0)));
}

#[test]
fn test_load_object_missing_value_errors() {
    let object_hash = blake3::hash(b"absent");
    let mut builder = BytecodeBuilder::new();
    builder.emit_load_object(object_hash);
    builder.emit(Opcode::Return);
    let func = builder.build(0, 0);
    let hash = func.hash;

    let mut vm = Vm::new();
    vm.load_function(func);
    // No value loaded: the reference dangles.
    assert_eq!(
        vm.call(&hash, vec![]),
        Err(ambient_ability::VmError::UnknownObject(object_hash))
    );
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
            expected: "two numbers or two strings",
            got: "Number",
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

/// Build a recursive factorial function using `FunctionBuilder`.
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

/// Build a recursive fibonacci function using `FunctionBuilder`.
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
            .push(f64::from(u32::try_from(n).unwrap()))
            .call_func(hash, 1)
            .run();

        assert_eq!(result, Ok(Value::Number(*exp)), "fib({n}) should be {exp}");
    }
}

// =========================================================================
// Milestone 2: Abilities and Handlers
// =========================================================================

use crate::test_utils::test_method_ref;

/// A default implementation `fn(x) { x * 2 }` under a stable test hash.
fn doubling_impl() -> crate::bytecode::CompiledFunction {
    FunctionBuilder::new("test::double_impl")
        .with_locals(1)
        .with_params(1)
        .load_local(0)
        .push(2.0)
        .mul()
        .build()
}

#[test]
fn test_suspend_creates_ability_value() {
    let method = test_method_ref(1, 0, None);
    let expected_key = method.method_key();
    let expected_id = method.ability_id;
    VmTest::new()
        .push(42.0)
        .suspend(&method, 1)
        .expect_suspended(move |ability| {
            assert_eq!(ability.ability_id, expected_id);
            assert_eq!(ability.method, expected_key);
            assert_eq!(ability.args.len(), 1);
            assert_eq!(ability.args[0], Value::Number(42.0));
        });
}

#[test]
fn test_unhandled_perform_runs_default_impl() {
    // No handler in scope: the perform calls the method's default
    // implementation as a plain function with the suspended arguments.
    let implementation = doubling_impl();
    let method = test_method_ref(2, 0, Some(implementation.hash));

    VmTest::new()
        .with_function(implementation)
        .push(21.0)
        .suspend(&method, 1)
        .perform()
        .expect_number(42.0);
}

#[test]
fn test_bytecode_handler_overrides_default_impl() {
    // The default impl would return 999.0, but an installed handler wins.
    let implementation = FunctionBuilder::new("test::default_999")
        .with_params(1)
        .with_locals(1)
        .push(999.0)
        .build();
    let method = test_method_ref(2, 0, Some(implementation.hash));

    let handler = FunctionBuilder::new("test::override_handler")
        .with_locals(2)
        .with_params(2)
        .load_local(0) // continuation
        .push(42.0) // resume value
        .resume()
        .build();
    let handler_hash = handler.hash;

    VmTest::new()
        .with_function(implementation)
        .with_function(handler)
        .handle(&method, handler_hash)
        .push(5.0)
        .suspend(&method, 1)
        .perform()
        .unhandle()
        .expect_number(42.0);
}

#[test]
fn test_uncovered_method_falls_through_to_default_impl() {
    // A handler covering method A must not swallow a perform of method B
    // on the same ability: B falls through and runs its default impl.
    let implementation = doubling_impl();
    let method_a = test_method_ref(2, 0, Some(implementation.hash));
    let method_b = test_method_ref(2, 1, Some(implementation.hash));

    let handler = FunctionBuilder::new("test::a_handler")
        .with_locals(2)
        .with_params(2)
        .load_local(0)
        .push(1000.0)
        .resume()
        .build();
    let handler_hash = handler.hash;

    VmTest::new()
        .with_function(implementation)
        .with_function(handler)
        .handle(&method_a, handler_hash)
        .push(21.0)
        .suspend(&method_b, 1)
        .perform()
        .unhandle()
        .expect_number(42.0);
}

#[test]
fn test_unhandled_abstract_method_errors() {
    // An abstract method (no default implementation) with no handler in
    // scope is a hard error. Only never-returning methods may stay
    // abstract (e.g. `Drain::requested`), but the VM rule is general.
    let method = test_method_ref(1, 0, None);
    VmTest::new()
        .push(42.0)
        .suspend(&method, 1)
        .perform()
        .expect_error(VmError::UnhandledAbility {
            ability_id: method.ability_id,
            method: method.method_key(),
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
    let method = test_method_ref(2, 0, None);

    VmTest::new()
        .with_function(handler)
        .handle(&method, handler_hash)
        .push(5.0)
        .suspend(&method, 1)
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
    let method = test_method_ref(2, 0, None);

    VmTest::new()
        .with_function(handler)
        .handle(&method, handler_hash)
        .push(5.0)
        .suspend(&method, 1)
        .perform()
        .unhandle()
        .expect_number(1.0);
}

#[test]
fn test_multiple_ability_calls() {
    // Three performs in a row, each landing on the default impl.
    let implementation = doubling_impl();
    let method = test_method_ref(1, 0, Some(implementation.hash));

    VmTest::new()
        .with_function(implementation)
        .push(1.0)
        .suspend(&method, 1)
        .perform()
        .pop()
        .push(2.0)
        .suspend(&method, 1)
        .perform()
        .pop()
        .push(3.0)
        .suspend(&method, 1)
        .perform()
        .expect_number(6.0);
}

#[test]
fn test_perform_expected_type_error() {
    VmTest::new()
        .push(42.0)
        .perform()
        .expect_error(VmError::ExpectedSuspendedAbility { got: "Number" });
}

// =========================================================================
// Milestone 3: Abilities as Values
// =========================================================================

#[test]
fn test_ability_with_multiple_args() {
    // Default impl `fn(a, b) { a + b }`.
    let implementation = FunctionBuilder::new("test::add_impl")
        .with_locals(2)
        .with_params(2)
        .load_local(0)
        .load_local(1)
        .add()
        .build();
    let method = test_method_ref(2, 1, Some(implementation.hash));

    VmTest::new()
        .with_function(implementation)
        .push(10.0)
        .push(32.0)
        .suspend(&method, 2)
        .perform()
        .expect_number(42.0);
}

#[test]
fn test_ability_stored_in_variable() {
    let implementation = doubling_impl();
    let method = test_method_ref(2, 0, Some(implementation.hash));

    VmTest::new()
        .with_function(implementation)
        .with_locals(1)
        .push(21.0)
        .suspend(&method, 1)
        .store_local(0)
        .pop()
        .push(999.0)
        .pop()
        .load_local(0)
        .perform()
        .expect_number(42.0);
}

#[test]
fn test_ability_stored_in_tuple() {
    let implementation = doubling_impl();
    let method = test_method_ref(2, 0, Some(implementation.hash));

    VmTest::new()
        .with_function(implementation)
        .with_locals(1)
        .push(21.0)
        .suspend(&method, 1)
        .push_str("label")
        .make_tuple(2)
        .store_local(0)
        .pop()
        .load_local(0)
        .tuple_get(0)
        .perform()
        .expect_number(42.0);
}

#[test]
fn test_ability_passed_to_function() {
    // perform_ability(op) = op!
    let implementation = doubling_impl();
    let method = test_method_ref(2, 0, Some(implementation.hash));
    let perform_fn = FunctionBuilder::new("test::perform_ability")
        .with_locals(1)
        .with_params(1)
        .load_local(0)
        .perform()
        .build();
    let perform_hash = perform_fn.hash;

    VmTest::new()
        .with_function(implementation)
        .with_function(perform_fn)
        .push(21.0)
        .suspend(&method, 1)
        .call_func(perform_hash, 1)
        .expect_number(42.0);
}

#[test]
fn test_multiple_abilities_different_order() {
    // op1 = double(10), op2 = double(21), perform op2
    let implementation = doubling_impl();
    let method = test_method_ref(2, 0, Some(implementation.hash));

    VmTest::new()
        .with_function(implementation)
        .with_locals(2)
        .push(10.0)
        .suspend(&method, 1)
        .store_local(0)
        .pop()
        .push(21.0)
        .suspend(&method, 1)
        .store_local(1)
        .pop()
        .load_local(1)
        .perform()
        .expect_number(42.0);
}

#[test]
fn test_ability_equality() {
    let method = test_method_ref(2, 0, None);
    VmTest::new()
        .with_locals(2)
        .push(42.0)
        .suspend(&method, 1)
        .store_local(0)
        .pop()
        .push(42.0)
        .suspend(&method, 1)
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
    let implementation = doubling_impl();
    let method = test_method_ref(2, 0, Some(implementation.hash));
    let creator_fn = FunctionBuilder::new("test::create_double_op")
        .with_locals(1)
        .with_params(1)
        .load_local(0)
        .suspend(&method, 1)
        .build();
    let creator_hash = creator_fn.hash;

    VmTest::new()
        .with_locals(1)
        .with_function(implementation)
        .with_function(creator_fn)
        .push(21.0)
        .call_func(creator_hash, 1)
        .store_local(0)
        .pop()
        .load_local(0)
        .perform()
        .expect_number(42.0);
}

#[test]
fn test_make_handler_creates_handler_value() {
    let method_a = test_method_ref(42, 0, None);

    // Create a simple handler method function that returns unit.
    let mut handler_builder = BytecodeBuilder::new();
    handler_builder.emit_const(Value::Unit);
    handler_builder.emit(Opcode::Return);
    let handler_func = handler_builder.build(2, 2);
    let handler_hash = handler_func.hash;

    // Create main function that makes a handler and returns it.
    let mut builder = BytecodeBuilder::new();

    // Emit MakeHandler: 1 method, 0 captures.
    builder.emit_make_handler(method_a.ability_id, &[(method_a.clone(), handler_hash)], 0);

    // Return the handler value.
    builder.emit(Opcode::Return);

    let main_func = builder.build(0, 0);
    let main_hash = main_func.hash;

    let mut vm = Vm::new();
    vm.load_function(handler_func);
    vm.load_function(main_func);

    let result = vm.call(&main_hash, vec![]);

    // Should return a handler value.
    assert!(result.is_ok(), "Should succeed: {result:?}");
    if let Ok(Value::Handler(handler)) = result {
        assert_eq!(handler.ability_id, method_a.ability_id);
        assert!(handler.handles_method(method_a.method_key()));
        assert_eq!(handler.methods.len(), 1);
    } else {
        panic!("Expected Handler value, got {result:?}");
    }
}

#[test]
fn test_make_handler_with_multiple_methods() {
    let method_a = test_method_ref(42, 0, None);
    let method_b = test_method_ref(42, 1, None);

    // Create handler method functions.
    let mut first_builder = BytecodeBuilder::new();
    first_builder.emit_const(Value::Unit);
    first_builder.emit(Opcode::Return);
    let first_func = first_builder.build(2, 2);
    let first_hash = first_func.hash;

    let mut second_builder = BytecodeBuilder::new();
    second_builder.emit_const(Value::Unit);
    second_builder.emit(Opcode::Return);
    let second_func = second_builder.build(2, 2);
    let second_hash = second_func.hash;

    // Create main function that makes a handler with 2 methods.
    let mut builder = BytecodeBuilder::new();
    builder.emit_make_handler(
        method_a.ability_id,
        &[
            (method_a.clone(), first_hash),
            (method_b.clone(), second_hash),
        ],
        0,
    );
    builder.emit(Opcode::Return);

    let main_func = builder.build(0, 0);
    let main_hash = main_func.hash;

    let mut vm = Vm::new();
    vm.load_function(first_func);
    vm.load_function(second_func);
    vm.load_function(main_func);

    let result = vm.call(&main_hash, vec![]);

    assert!(result.is_ok(), "Should succeed: {result:?}");
    if let Ok(Value::Handler(handler)) = result {
        assert_eq!(handler.ability_id, method_a.ability_id);
        assert!(handler.handles_method(method_a.method_key()));
        assert!(handler.handles_method(method_b.method_key()));
        assert_eq!(handler.methods.len(), 2);
    } else {
        panic!("Expected Handler value, got {result:?}");
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
        panic!("Expected List, got {result:?}");
    }
}

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

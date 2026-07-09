//! Test utilities for VM builder tests.
//!
//! Provides a fluent DSL for writing concise, readable VM tests.
//!
//! # Example
//!
//! ```ignore
//! use crate::test_utils::VmTest;
//!
//! #[test]
//! fn test_add() {
//!     VmTest::new()
//!         .push(10.0)
//!         .push(32.0)
//!         .add()
//!         .expect_number(42.0);
//! }
//! ```

// Fluent builder methods (`.neg()`, `.not()`, ...) read better than the
// operator traits clippy suggests; renaming them would hurt the DSL.
#![allow(clippy::should_implement_trait)]
// Test-only DSL: assertion helpers panic by design, so documenting every
// panic path adds no value here.
#![allow(clippy::missing_panics_doc)]
// Returning Result from a test helper is deliberate; an `# Errors` section
// on test-only code adds no value.
#![allow(clippy::missing_errors_doc)]

use std::sync::Arc;

use crate::bytecode::{BytecodeBuilder, CompiledFunction, Opcode};
use crate::types::AbilityId;
use crate::value::{AbilityMethodRef, SuspendedAbility, Value};
use crate::vm::{Vm, VmError};

/// A synthetic ability-method reference for VM tests: recognizable
/// identity bytes, no relation to any real declaration. `impl_fn` is the
/// default implementation an unhandled perform calls (load it as an aux
/// function), or `None` for an abstract method.
#[must_use]
pub fn test_method_ref(
    ability_byte: u8,
    method_byte: u8,
    impl_fn: Option<blake3::Hash>,
) -> AbilityMethodRef {
    AbilityMethodRef {
        ability_id: AbilityId::from_bytes([ability_byte; 32]),
        ability_uuid: uuid::Uuid::from_u128(u128::from(ability_byte)),
        signature: ambient_core::SignatureHash::from_bytes([method_byte; 32]),
        impl_fn,
    }
}

/// A fluent test builder for VM tests.
///
/// Provides a chainable API for constructing and running bytecode tests
/// with minimal boilerplate.
pub struct VmTest {
    builder: BytecodeBuilder,
    locals: u16,
    params: u8,
    aux_functions: Vec<CompiledFunction>,
    call_args: Vec<Value>,
    predetermined_hash: Option<blake3::Hash>,
}

impl Default for VmTest {
    fn default() -> Self {
        Self::new()
    }
}

impl VmTest {
    /// Create a new test builder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            builder: BytecodeBuilder::new(),
            locals: 0,
            params: 0,
            aux_functions: Vec::new(),
            call_args: Vec::new(),
            predetermined_hash: None,
        }
    }

    // =========================================================================
    // Configuration
    // =========================================================================

    /// Set the number of local variable slots.
    #[must_use]
    pub fn with_locals(mut self, count: u16) -> Self {
        self.locals = count;
        self
    }

    /// Set the number of parameters.
    #[must_use]
    pub fn with_params(mut self, count: u8) -> Self {
        self.params = count;
        self
    }

    /// Set a predetermined hash for self-recursive functions.
    #[must_use]
    pub fn with_hash(mut self, name: &str) -> Self {
        self.predetermined_hash = Some(blake3::hash(name.as_bytes()));
        self
    }

    /// Add arguments to pass when calling the main function.
    #[must_use]
    pub fn with_args(mut self, args: Vec<Value>) -> Self {
        self.call_args = args;
        self
    }

    /// Add an auxiliary function to be loaded into the VM.
    #[must_use]
    pub fn with_function(mut self, func: CompiledFunction) -> Self {
        self.aux_functions.push(func);
        self
    }

    // =========================================================================
    // Constants
    // =========================================================================

    /// Push a number constant.
    #[must_use]
    pub fn push(mut self, n: f64) -> Self {
        self.builder.emit_const(Value::Number(n));
        self
    }

    /// Push a boolean constant.
    #[must_use]
    pub fn push_bool(mut self, b: bool) -> Self {
        self.builder.emit_const(Value::Bool(b));
        self
    }

    /// Push a string constant.
    #[must_use]
    pub fn push_str(mut self, s: &str) -> Self {
        self.builder.emit_const(Value::string(s));
        self
    }

    /// Push a unit value.
    #[must_use]
    pub fn push_unit(mut self) -> Self {
        self.builder.emit_const(Value::Unit);
        self
    }

    /// Push an arbitrary value.
    #[must_use]
    pub fn push_value(mut self, v: Value) -> Self {
        self.builder.emit_const(v);
        self
    }

    // =========================================================================
    // Arithmetic
    // =========================================================================

    /// Add two numbers.
    #[must_use]
    pub fn add(mut self) -> Self {
        self.builder.emit(Opcode::Add);
        self
    }

    /// Subtract top from second.
    #[must_use]
    pub fn sub(mut self) -> Self {
        self.builder.emit(Opcode::Sub);
        self
    }

    /// Multiply two numbers.
    #[must_use]
    pub fn mul(mut self) -> Self {
        self.builder.emit(Opcode::Mul);
        self
    }

    /// Divide second by top.
    #[must_use]
    pub fn div(mut self) -> Self {
        self.builder.emit(Opcode::Div);
        self
    }

    /// Modulo second by top.
    #[must_use]
    pub fn modulo(mut self) -> Self {
        self.builder.emit(Opcode::Mod);
        self
    }

    /// Negate top of stack.
    #[must_use]
    pub fn neg(mut self) -> Self {
        self.builder.emit(Opcode::Neg);
        self
    }

    // =========================================================================
    // Comparison
    // =========================================================================

    /// Test equality.
    #[must_use]
    pub fn eq(mut self) -> Self {
        self.builder.emit(Opcode::Eq);
        self
    }

    /// Test inequality.
    #[must_use]
    pub fn ne(mut self) -> Self {
        self.builder.emit(Opcode::Ne);
        self
    }

    /// Test less than.
    #[must_use]
    pub fn lt(mut self) -> Self {
        self.builder.emit(Opcode::Lt);
        self
    }

    /// Test less or equal.
    #[must_use]
    pub fn le(mut self) -> Self {
        self.builder.emit(Opcode::Le);
        self
    }

    /// Test greater than.
    #[must_use]
    pub fn gt(mut self) -> Self {
        self.builder.emit(Opcode::Gt);
        self
    }

    /// Test greater or equal.
    #[must_use]
    pub fn ge(mut self) -> Self {
        self.builder.emit(Opcode::Ge);
        self
    }

    // =========================================================================
    // Logic
    // =========================================================================

    /// Logical AND.
    #[must_use]
    pub fn and(mut self) -> Self {
        self.builder.emit(Opcode::And);
        self
    }

    /// Logical OR.
    #[must_use]
    pub fn or(mut self) -> Self {
        self.builder.emit(Opcode::Or);
        self
    }

    /// Logical NOT.
    #[must_use]
    pub fn not(mut self) -> Self {
        self.builder.emit(Opcode::Not);
        self
    }

    // =========================================================================
    // Stack
    // =========================================================================

    /// Pop and discard top of stack.
    #[must_use]
    pub fn pop(mut self) -> Self {
        self.builder.emit(Opcode::Pop);
        self
    }

    /// Duplicate top of stack.
    #[must_use]
    pub fn dup(mut self) -> Self {
        self.builder.emit(Opcode::Dup);
        self
    }

    // =========================================================================
    // Local Variables
    // =========================================================================

    /// Store top of stack to local slot.
    #[must_use]
    pub fn store_local(mut self, slot: u16) -> Self {
        self.builder.emit_u16(Opcode::StoreLocal, slot);
        self
    }

    /// Load from local slot.
    #[must_use]
    pub fn load_local(mut self, slot: u16) -> Self {
        self.builder.emit_u16(Opcode::LoadLocal, slot);
        self
    }

    // =========================================================================
    // Data Structures
    // =========================================================================

    /// Make a tuple from top N stack values.
    #[must_use]
    pub fn make_tuple(mut self, arity: u8) -> Self {
        self.builder.emit_u8(Opcode::MakeTuple, arity);
        self
    }

    /// Get tuple element by index.
    #[must_use]
    pub fn tuple_get(mut self, index: u8) -> Self {
        self.builder.emit_u8(Opcode::TupleGet, index);
        self
    }

    /// Make a record from top N field-value pairs.
    #[must_use]
    pub fn make_record(mut self, field_count: u8) -> Self {
        self.builder.emit_u8(Opcode::MakeRecord, field_count);
        self
    }

    /// Get a field from a record.
    #[must_use]
    pub fn record_get(mut self, field: &str) -> Self {
        let idx = self.builder.add_constant(Value::string(field));
        self.builder.emit_u16(Opcode::RecordGet, idx);
        self
    }

    // =========================================================================
    // Function Calls
    // =========================================================================

    /// Call a function by hash.
    #[must_use]
    pub fn call_func(mut self, hash: blake3::Hash, arg_count: u8) -> Self {
        self.builder.emit_call(hash, arg_count);
        self
    }

    /// Emit return instruction (usually automatic, but available for explicit use).
    #[must_use]
    pub fn ret(mut self) -> Self {
        self.builder.emit(Opcode::Return);
        self
    }

    // =========================================================================
    // Abilities
    // =========================================================================

    /// Create a suspended ability value.
    #[must_use]
    pub fn suspend(mut self, method: &AbilityMethodRef, arg_count: u8) -> Self {
        self.builder.emit_suspend(method.clone(), arg_count);
        self
    }

    /// Perform a suspended ability.
    #[must_use]
    pub fn perform(mut self) -> Self {
        self.builder.emit(Opcode::Perform);
        self
    }

    // =========================================================================
    // Handlers
    // =========================================================================

    /// Install a bytecode handler for one method of an ability.
    ///
    /// The arm function is packaged into a capture-free single-method
    /// `HandlerValue` and installed with `HandleWithValue` — the sole
    /// handler-install path. The current frame becomes the delimitation
    /// boundary.
    #[must_use]
    pub fn handle(mut self, method: &AbilityMethodRef, handler_hash: blake3::Hash) -> Self {
        self.builder
            .emit_make_handler(method.ability_id, &[(method.clone(), handler_hash)], 0);
        self.builder.emit_handle_with_value();
        self
    }

    /// Remove the most recent handler.
    #[must_use]
    pub fn unhandle(mut self) -> Self {
        self.builder.emit(Opcode::Unhandle);
        self
    }

    // =========================================================================
    // Escape Hatch
    // =========================================================================

    /// Access the raw builder for complex control flow.
    #[must_use]
    pub fn with_builder<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut BytecodeBuilder),
    {
        f(&mut self.builder);
        self
    }

    // =========================================================================
    // Execution
    // =========================================================================

    /// Build and run the test, returning the result.
    pub fn run(mut self) -> Result<Value, VmError> {
        // Emit Return if not already done
        self.builder.emit(Opcode::Return);

        // Build the function
        let mut func = self.builder.build(self.locals, self.params);

        // Override hash if predetermined
        if let Some(hash) = self.predetermined_hash {
            func.hash = hash;
        }

        let main_hash = func.hash;

        // Create VM and load functions
        let mut vm = Vm::new();
        for aux in self.aux_functions {
            vm.load_function(aux);
        }
        vm.load_function(func);

        // Call and return result
        vm.call(&main_hash, self.call_args)
    }

    // =========================================================================
    // Assertions
    // =========================================================================

    /// Assert the result equals an expected value.
    pub fn expect(self, expected: Value) {
        let result = self.run();
        assert_eq!(result, Ok(expected));
    }

    /// Assert the result is a specific number.
    pub fn expect_number(self, n: f64) {
        self.expect(Value::Number(n));
    }

    /// Assert the result is a specific boolean.
    pub fn expect_bool(self, b: bool) {
        self.expect(Value::Bool(b));
    }

    /// Assert the result is unit.
    pub fn expect_unit(self) {
        self.expect(Value::Unit);
    }

    /// Assert the result is a specific string.
    pub fn expect_string(self, s: &str) {
        self.expect(Value::string(s));
    }

    /// Assert the result is an error.
    pub fn expect_error(self, err: VmError) {
        let result = self.run();
        assert_eq!(result, Err(err));
    }

    /// Assert the result matches a predicate.
    pub fn expect_match<F>(self, predicate: F)
    where
        F: FnOnce(&Result<Value, VmError>) -> bool,
    {
        let result = self.run();
        assert!(
            predicate(&result),
            "Result did not match predicate: {result:?}"
        );
    }

    /// Assert the result is a tuple and apply custom assertions.
    pub fn expect_tuple<F>(self, check: F)
    where
        F: FnOnce(&[Value]),
    {
        let result = self.run();
        match result {
            Ok(Value::Tuple(elements)) => check(&elements),
            other => panic!("Expected tuple, got {other:?}"),
        }
    }

    /// Assert the result is a record and apply custom assertions.
    pub fn expect_record<F>(self, check: F)
    where
        F: FnOnce(&std::collections::HashMap<Arc<str>, Value>),
    {
        let result = self.run();
        match result {
            Ok(Value::Record(fields)) => check(&fields),
            other => panic!("Expected record, got {other:?}"),
        }
    }

    /// Assert the result is a suspended ability and apply custom assertions.
    pub fn expect_suspended<F>(self, check: F)
    where
        F: FnOnce(&SuspendedAbility),
    {
        let result = self.run();
        match result {
            Ok(Value::SuspendedAbility(ability)) => check(&ability),
            other => panic!("Expected SuspendedAbility, got {other:?}"),
        }
    }
}

// =============================================================================
// FunctionBuilder - For auxiliary functions
// =============================================================================

/// Builder for creating auxiliary functions (handlers, helpers).
pub struct FunctionBuilder {
    builder: BytecodeBuilder,
    hash: blake3::Hash,
    locals: u16,
    params: u8,
}

impl FunctionBuilder {
    /// Create a new function builder with a predetermined hash.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            builder: BytecodeBuilder::new(),
            hash: blake3::hash(name.as_bytes()),
            locals: 0,
            params: 0,
        }
    }

    /// Get the function's hash.
    #[must_use]
    pub fn hash(&self) -> blake3::Hash {
        self.hash
    }

    /// Set the number of local variable slots.
    #[must_use]
    pub fn with_locals(mut self, count: u16) -> Self {
        self.locals = count;
        self
    }

    /// Set the number of parameters.
    #[must_use]
    pub fn with_params(mut self, count: u8) -> Self {
        self.params = count;
        self
    }

    // =========================================================================
    // Constants
    // =========================================================================

    /// Push a number constant.
    #[must_use]
    pub fn push(mut self, n: f64) -> Self {
        self.builder.emit_const(Value::Number(n));
        self
    }

    /// Push a boolean constant.
    #[must_use]
    pub fn push_bool(mut self, b: bool) -> Self {
        self.builder.emit_const(Value::Bool(b));
        self
    }

    /// Push a string constant.
    #[must_use]
    pub fn push_str(mut self, s: &str) -> Self {
        self.builder.emit_const(Value::string(s));
        self
    }

    /// Push unit.
    #[must_use]
    pub fn push_unit(mut self) -> Self {
        self.builder.emit_const(Value::Unit);
        self
    }

    /// Push an arbitrary value.
    #[must_use]
    pub fn push_value(mut self, v: Value) -> Self {
        self.builder.emit_const(v);
        self
    }

    // =========================================================================
    // Arithmetic
    // =========================================================================

    /// Add two numbers.
    #[must_use]
    pub fn add(mut self) -> Self {
        self.builder.emit(Opcode::Add);
        self
    }

    /// Subtract.
    #[must_use]
    pub fn sub(mut self) -> Self {
        self.builder.emit(Opcode::Sub);
        self
    }

    /// Multiply.
    #[must_use]
    pub fn mul(mut self) -> Self {
        self.builder.emit(Opcode::Mul);
        self
    }

    /// Divide.
    #[must_use]
    pub fn div(mut self) -> Self {
        self.builder.emit(Opcode::Div);
        self
    }

    /// Negate.
    #[must_use]
    pub fn neg(mut self) -> Self {
        self.builder.emit(Opcode::Neg);
        self
    }

    // =========================================================================
    // Comparison
    // =========================================================================

    /// Test less or equal.
    #[must_use]
    pub fn le(mut self) -> Self {
        self.builder.emit(Opcode::Le);
        self
    }

    // =========================================================================
    // Local Variables
    // =========================================================================

    /// Load from local slot.
    #[must_use]
    pub fn load_local(mut self, slot: u16) -> Self {
        self.builder.emit_u16(Opcode::LoadLocal, slot);
        self
    }

    /// Store to local slot.
    #[must_use]
    pub fn store_local(mut self, slot: u16) -> Self {
        self.builder.emit_u16(Opcode::StoreLocal, slot);
        self
    }

    // =========================================================================
    // Stack
    // =========================================================================

    /// Pop and discard.
    #[must_use]
    pub fn pop(mut self) -> Self {
        self.builder.emit(Opcode::Pop);
        self
    }

    // =========================================================================
    // Abilities
    // =========================================================================

    /// Create a suspended ability.
    #[must_use]
    pub fn suspend(mut self, method: &AbilityMethodRef, arg_count: u8) -> Self {
        self.builder.emit_suspend(method.clone(), arg_count);
        self
    }

    /// Perform ability.
    #[must_use]
    pub fn perform(mut self) -> Self {
        self.builder.emit(Opcode::Perform);
        self
    }

    /// Resume a continuation.
    #[must_use]
    pub fn resume(mut self) -> Self {
        self.builder.emit(Opcode::Resume);
        self
    }

    // =========================================================================
    // Function Calls
    // =========================================================================

    /// Call a function by hash.
    #[must_use]
    pub fn call_func(mut self, hash: blake3::Hash, arg_count: u8) -> Self {
        self.builder.emit_call(hash, arg_count);
        self
    }

    /// Emit return instruction.
    #[must_use]
    pub fn ret(mut self) -> Self {
        self.builder.emit(Opcode::Return);
        self
    }

    // =========================================================================
    // Escape Hatch
    // =========================================================================

    /// Access raw builder for complex control flow.
    #[must_use]
    pub fn with_builder<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut BytecodeBuilder),
    {
        f(&mut self.builder);
        self
    }

    // =========================================================================
    // Build
    // =========================================================================

    /// Build the compiled function.
    #[must_use]
    pub fn build(mut self) -> CompiledFunction {
        self.builder.emit(Opcode::Return);
        let mut func = self.builder.build(self.locals, self.params);
        func.hash = self.hash;
        func
    }
}

// =============================================================================
// Capture - For side effect testing
// =============================================================================

/// Helper for capturing side effects in handlers.
///
/// Uses `Arc<Mutex<>>` for thread safety with Send + Sync handlers.
///
/// # Example
///
/// ```ignore
/// let capture = Capture::<f64>::new();
///
/// VmTest::new()
///     .push(42.0)
///     .suspend(CONSOLE, PRINT, 1)
///     .perform()
///     .with_host_handler(CONSOLE, PRINT, {
///         let log = capture.clone_inner();
///         move |ability| {
///             if let Value::Number(n) = &ability.args[0] {
///                 log.lock().expect("lock").push(*n);
///             }
///             Ok(Value::Unit)
///         }
///     })
///     .expect_unit();
///
/// capture.assert_eq(&[42.0]);
/// ```
#[derive(Debug)]
pub struct Capture<T> {
    inner: Arc<std::sync::Mutex<Vec<T>>>,
}

impl<T> Default for Capture<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Capture<T> {
    /// Create a new empty capture.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// Get a clone of the inner Arc for use in closures.
    #[must_use]
    pub fn clone_inner(&self) -> Arc<std::sync::Mutex<Vec<T>>> {
        Arc::clone(&self.inner)
    }

    /// Get a reference to captured values.
    pub fn values(&self) -> std::sync::MutexGuard<'_, Vec<T>> {
        self.inner.lock().expect("lock poisoned")
    }
}

impl<T: std::fmt::Debug> Capture<T> {
    /// Get the captured values.
    ///
    /// # Panics
    ///
    /// Panics if the Capture still has other references.
    #[must_use]
    pub fn into_vec(self) -> Vec<T> {
        Arc::try_unwrap(self.inner)
            .expect("Capture still has references")
            .into_inner()
            .expect("lock poisoned")
    }
}

impl<T: PartialEq + std::fmt::Debug> Capture<T> {
    /// Assert the captured values equal expected.
    pub fn assert_eq(&self, expected: &[T]) {
        assert_eq!(*self.inner.lock().expect("lock"), expected);
    }
}

impl<T> Clone for Capture<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

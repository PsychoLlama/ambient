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

use std::sync::Arc;

use crate::bytecode::{BytecodeBuilder, CompiledFunction, Opcode};
use crate::value::{SuspendedAbility, Value};
use crate::vm::{HostHandler, Vm, VmError};

/// A fluent test builder for VM tests.
///
/// Provides a chainable API for constructing and running bytecode tests
/// with minimal boilerplate.
pub struct VmTest {
    builder: BytecodeBuilder,
    locals: u16,
    params: u8,
    aux_functions: Vec<CompiledFunction>,
    host_handlers: Vec<(u16, u16, HostHandler)>,
    call_args: Vec<Value>,
    predetermined_hash: Option<blake3::Hash>,
    pending_handles: Vec<PendingHandle>,
}

struct PendingHandle {
    #[allow(dead_code)]
    ability_id: u16,
    jump_offset: usize,
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
            host_handlers: Vec::new(),
            call_args: Vec::new(),
            predetermined_hash: None,
            pending_handles: Vec::new(),
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
    pub fn suspend(mut self, ability_id: u16, method_id: u16, arg_count: u8) -> Self {
        self.builder.emit_suspend(ability_id, method_id, arg_count);
        self
    }

    /// Perform a suspended ability.
    #[must_use]
    pub fn perform(mut self) -> Self {
        self.builder.emit(Opcode::Perform);
        self
    }

    /// Perform multiple suspended abilities concurrently and collect all results.
    #[must_use]
    pub fn async_all(mut self, count: u8) -> Self {
        self.builder.emit_u8(Opcode::AsyncAll, count);
        self
    }

    /// Race multiple suspended abilities, returning the first to complete.
    #[must_use]
    pub fn async_race(mut self, count: u8) -> Self {
        self.builder.emit_u8(Opcode::AsyncRace, count);
        self
    }

    // =========================================================================
    // Handlers
    // =========================================================================

    /// Register a host handler.
    #[must_use]
    pub fn with_host_handler<F>(mut self, ability_id: u16, method_id: u16, handler: F) -> Self
    where
        F: Fn(&SuspendedAbility) -> Result<Value, VmError> + Send + Sync + 'static,
    {
        self.host_handlers
            .push((ability_id, method_id, Box::new(handler)));
        self
    }

    /// Install a bytecode handler for an ability.
    #[must_use]
    pub fn handle(mut self, ability_id: u16, handler_hash: blake3::Hash) -> Self {
        let jump_offset = self.builder.emit_handle(ability_id, handler_hash);
        self.pending_handles.push(PendingHandle {
            ability_id,
            jump_offset,
        });
        self
    }

    /// Remove the most recent handler and patch its jump target.
    #[must_use]
    pub fn unhandle(mut self) -> Self {
        self.builder.emit(Opcode::Unhandle);
        if let Some(pending) = self.pending_handles.pop() {
            self.builder.patch_handle(pending.jump_offset);
        }
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

        // Register host handlers
        for (ability_id, method_id, handler) in self.host_handlers {
            vm.register_host_handler(ability_id, method_id, handler);
        }

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
        assert!(predicate(&result), "Result did not match predicate: {result:?}");
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
    pub fn suspend(mut self, ability_id: u16, method_id: u16, arg_count: u8) -> Self {
        self.builder.emit_suspend(ability_id, method_id, arg_count);
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

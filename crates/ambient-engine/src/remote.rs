//! Remote execution support for the Ambient VM.
//!
//! This module provides the infrastructure for executing functions remotely by
//! serializing code and values, transferring them to another VM instance, and
//! receiving results back.
//!
//! # Architecture
//!
//! The remote execution model follows the wire protocol defined in the spec:
//!
//! ```text
//! Client                          Server
//!   |                               |
//!   |-- Execute(hash, args) ------->|
//!   |                               |
//!   |<-- NeedDeps([hash1, hash2]) --|  (if server missing dependencies)
//!   |                               |
//!   |-- Provide([fn1, fn2]) ------->|
//!   |                               |
//!   |<-- Result(value) -------------|  (or Error)
//! ```
//!
//! # Same-Process vs Network
//!
//! This module implements same-process remote execution (Milestone 5).
//! The `Executor` abstraction is designed to later support network-based
//! remote execution (Milestone 6) by implementing different transport layers.

#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

use serde::{Deserialize, Serialize};

use crate::store::Store;
use crate::value::Value;
use crate::vm::{Vm, VmError};

/// A request to execute a function remotely.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRequest {
    /// Hash of the function to execute.
    pub function_hash: blake3::Hash,

    /// Arguments to pass to the function.
    pub arguments: Vec<Value>,

    /// Serialized store containing the function and its dependencies.
    ///
    /// This is the JSON-serialized portable store format.
    pub store_data: Vec<u8>,
}

impl ExecutionRequest {
    /// Create a new execution request.
    ///
    /// # Arguments
    ///
    /// * `function_hash` - Hash of the function to execute
    /// * `arguments` - Arguments to pass to the function
    /// * `store` - Store containing the function and its dependencies
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be serialized.
    pub fn new(
        function_hash: blake3::Hash,
        arguments: Vec<Value>,
        store: &Store,
    ) -> Result<Self, RemoteError> {
        let store_data = store
            .serialize()
            .map_err(|e| RemoteError::Serialization(e.to_string()))?;

        Ok(Self {
            function_hash,
            arguments,
            store_data,
        })
    }
}

/// Response from a remote execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecutionResponse {
    /// Execution completed successfully.
    Success(Value),

    /// Execution failed with an error.
    Error(RemoteError),

    /// Server is missing some dependencies and needs them to proceed.
    NeedDependencies(Vec<blake3::Hash>),
}

/// Errors that can occur during remote execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RemoteError {
    /// Failed to serialize data.
    Serialization(String),

    /// Failed to deserialize data.
    Deserialization(String),

    /// VM execution failed.
    Execution(String),

    /// The requested function was not found.
    FunctionNotFound(blake3::Hash),

    /// Missing dependencies that could not be resolved.
    MissingDependencies(Vec<blake3::Hash>),
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialization(msg) => write!(f, "serialization error: {msg}"),
            Self::Deserialization(msg) => write!(f, "deserialization error: {msg}"),
            Self::Execution(msg) => write!(f, "execution error: {msg}"),
            Self::FunctionNotFound(hash) => write!(f, "function not found: {hash}"),
            Self::MissingDependencies(hashes) => {
                write!(f, "missing dependencies: ")?;
                for (i, hash) in hashes.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{hash}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for RemoteError {}

impl From<VmError> for RemoteError {
    fn from(err: VmError) -> Self {
        Self::Execution(err.to_string())
    }
}

/// A remote executor that can execute functions in an isolated VM instance.
///
/// The executor maintains its own store and VM, separate from the caller's.
/// Functions and their dependencies must be transferred to the executor
/// before execution.
///
/// # Example
///
/// ```ignore
/// use ambient_engine::remote::Executor;
/// use ambient_engine::store::Store;
///
/// // Create a function and its store
/// let mut store = Store::new();
/// let hash = store.add(my_function);
///
/// // Create an executor (represents "remote" VM)
/// let mut executor = Executor::new();
///
/// // Execute remotely
/// let result = executor.execute(hash, vec![], &store)?;
/// ```
#[derive(Default)]
pub struct Executor {
    /// The executor's local store of functions.
    store: Store,

    /// The executor's VM instance.
    vm: Vm,
}

impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Executor")
            .field("store", &self.store)
            .field("vm", &"<Vm>")
            .finish()
    }
}

impl Executor {
    /// Create a new executor with an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            store: Store::new(),
            vm: Vm::new(),
        }
    }

    /// Create an executor with pre-loaded functions.
    #[must_use]
    pub fn with_store(store: Store) -> Self {
        let mut vm = Vm::new();
        for hash in store.hashes() {
            if let Some(func) = store.get(&hash) {
                vm.load_function((*func).clone());
            }
        }
        Self { store, vm }
    }

    /// Get a reference to the executor's store.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Get a mutable reference to the executor's store.
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    /// Get a mutable reference to the executor's VM.
    ///
    /// This allows registering host ability handlers.
    pub fn vm_mut(&mut self) -> &mut Vm {
        &mut self.vm
    }

    /// Execute a function with the given arguments.
    ///
    /// This method handles the full execution protocol:
    /// 1. Receives the request with serialized store
    /// 2. Deserializes and merges the store
    /// 3. Loads new functions into the VM
    /// 4. Executes the function
    /// 5. Returns the result
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The store cannot be deserialized
    /// - The function is not found
    /// - There are missing dependencies
    /// - VM execution fails
    pub fn execute(&mut self, request: ExecutionRequest) -> ExecutionResponse {
        // Deserialize the incoming store
        let incoming_store = match Store::deserialize(&request.store_data) {
            Ok(s) => s,
            Err(e) => return ExecutionResponse::Error(RemoteError::Deserialization(e.to_string())),
        };

        // Merge the incoming store into our local store
        self.store.merge(&incoming_store);

        // Load any new functions into the VM
        for hash in incoming_store.hashes() {
            if let Some(func) = incoming_store.get(&hash) {
                self.vm.load_function((*func).clone());
            }
        }

        // Verify the function exists
        if !self.store.contains(&request.function_hash) {
            return ExecutionResponse::Error(RemoteError::FunctionNotFound(request.function_hash));
        }

        // Check for missing dependencies
        let missing = self.store.missing_dependencies(&request.function_hash);
        if !missing.is_empty() {
            return ExecutionResponse::NeedDependencies(missing);
        }

        // Execute the function
        match self.vm.call(&request.function_hash, request.arguments) {
            Ok(value) => ExecutionResponse::Success(value),
            Err(err) => ExecutionResponse::Error(RemoteError::from(err)),
        }
    }

    /// Execute a function directly from a store (convenience method).
    ///
    /// This is a simpler interface for same-process execution where you
    /// already have the store in memory.
    pub fn execute_from_store(
        &mut self,
        function_hash: blake3::Hash,
        arguments: Vec<Value>,
        store: &Store,
    ) -> Result<Value, RemoteError> {
        let request = ExecutionRequest::new(function_hash, arguments, store)?;

        match self.execute(request) {
            ExecutionResponse::Success(value) => Ok(value),
            ExecutionResponse::Error(err) => Err(err),
            ExecutionResponse::NeedDependencies(deps) => Err(RemoteError::MissingDependencies(deps)),
        }
    }
}

/// Client for sending execution requests to a remote executor.
///
/// The client handles the dependency negotiation protocol, automatically
/// providing missing dependencies when requested.
#[derive(Debug, Default)]
pub struct Client {
    /// The client's local store containing functions to execute.
    store: Store,
}

impl Client {
    /// Create a new client with an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a client with a pre-populated store.
    #[must_use]
    pub fn with_store(store: Store) -> Self {
        Self { store }
    }

    /// Get a mutable reference to the client's store.
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    /// Execute a function on a remote executor.
    ///
    /// This method handles the full protocol including dependency negotiation:
    /// 1. Extracts the function and its dependencies from the local store
    /// 2. Sends the execution request
    /// 3. If the executor needs more dependencies, provides them
    /// 4. Returns the final result
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The function is not in the local store
    /// - Required dependencies are missing locally
    /// - The remote execution fails
    pub fn execute_on(
        &self,
        executor: &mut Executor,
        function_hash: blake3::Hash,
        arguments: &[Value],
    ) -> Result<Value, RemoteError> {
        const MAX_ATTEMPTS: usize = 10; // Prevent infinite loops

        // Check if we have the function
        if !self.store.contains(&function_hash) {
            return Err(RemoteError::FunctionNotFound(function_hash));
        }

        // Extract the function and all its dependencies
        let subset = self.store.extract_with_dependencies(&function_hash);

        // Create the initial request
        let request = ExecutionRequest::new(function_hash, arguments.to_vec(), &subset)?;

        // Execute with dependency negotiation loop
        let mut response = executor.execute(request);

        // Handle NeedDependencies by providing more functions
        // (In practice this shouldn't happen if extract_with_dependencies works correctly,
        // but we handle it for robustness and future network scenarios)
        let mut attempts = 0;

        while let ExecutionResponse::NeedDependencies(needed) = &response {
            attempts += 1;
            if attempts > MAX_ATTEMPTS {
                return Err(RemoteError::MissingDependencies(needed.clone()));
            }

            // Try to provide the missing dependencies
            let mut additional = Store::new();
            for hash in needed {
                if let Some(func) = self.store.get(hash) {
                    additional.add((*func).clone());
                } else {
                    // We don't have this dependency either
                    return Err(RemoteError::MissingDependencies(vec![*hash]));
                }
            }

            // Send another request with the additional functions
            let request = ExecutionRequest::new(function_hash, arguments.to_vec(), &additional)?;
            response = executor.execute(request);
        }

        match response {
            ExecutionResponse::Success(value) => Ok(value),
            ExecutionResponse::Error(err) => Err(err),
            ExecutionResponse::NeedDependencies(_) => unreachable!("handled in loop above"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::{BytecodeBuilder, Opcode};

    /// Create a simple function that returns a constant.
    fn make_const_function(value: f64) -> crate::bytecode::CompiledFunction {
        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Number(value));
        builder.emit(Opcode::Return);
        builder.build(0, 0)
    }

    /// Create a function that adds two numbers.
    fn make_add_function() -> crate::bytecode::CompiledFunction {
        let mut builder = BytecodeBuilder::new();
        // Parameters are in locals 0 and 1
        builder.emit_u16(Opcode::LoadLocal, 0);
        builder.emit_u16(Opcode::LoadLocal, 1);
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);
        builder.build(2, 2) // 2 locals, 2 params
    }

    /// Create a function that calls another function.
    fn make_caller_function(callee_hash: blake3::Hash) -> crate::bytecode::CompiledFunction {
        let mut builder = BytecodeBuilder::new();
        // Push arguments for the callee
        builder.emit_const(Value::Number(10.0));
        builder.emit_const(Value::Number(20.0));
        // Call the function
        builder.emit_call(callee_hash, 2);
        builder.emit(Opcode::Return);
        builder.build_with_dependencies(0, 0, vec![callee_hash])
    }

    #[test]
    fn test_execution_request_creation() {
        let mut store = Store::new();
        let func = make_const_function(42.0);
        let hash = store.add(func);

        let request = ExecutionRequest::new(hash, vec![], &store).expect("should create request");
        assert_eq!(request.function_hash, hash);
        assert!(request.arguments.is_empty());
        assert!(!request.store_data.is_empty());
    }

    #[test]
    fn test_executor_simple_function() {
        // Create a function in the "client" store
        let mut store = Store::new();
        let func = make_const_function(42.0);
        let hash = store.add(func);

        // Create the executor (represents the "remote" side)
        let mut executor = Executor::new();

        // Execute the function
        let result = executor
            .execute_from_store(hash, vec![], &store)
            .expect("should execute");

        assert_eq!(result, Value::Number(42.0));
    }

    #[test]
    fn test_executor_function_with_args() {
        let mut store = Store::new();
        let func = make_add_function();
        let hash = store.add(func);

        let mut executor = Executor::new();
        let args = vec![Value::Number(10.0), Value::Number(32.0)];

        let result = executor
            .execute_from_store(hash, args, &store)
            .expect("should execute");

        assert_eq!(result, Value::Number(42.0));
    }

    #[test]
    fn test_executor_function_not_found() {
        let store = Store::new();
        let fake_hash = blake3::hash(b"nonexistent");

        let mut executor = Executor::new();
        let result = executor.execute_from_store(fake_hash, vec![], &store);

        assert!(matches!(result, Err(RemoteError::FunctionNotFound(_))));
    }

    #[test]
    fn test_executor_with_dependencies() {
        let mut store = Store::new();

        // Add the callee function (add)
        let add_func = make_add_function();
        let add_hash = store.add(add_func);

        // Add the caller function that depends on add
        let caller_func = make_caller_function(add_hash);
        let caller_hash = store.add(caller_func);

        // Execute on a fresh executor
        let mut executor = Executor::new();
        let result = executor
            .execute_from_store(caller_hash, vec![], &store)
            .expect("should execute with dependencies");

        // caller calls add(10, 20) = 30
        assert_eq!(result, Value::Number(30.0));
    }

    #[test]
    fn test_client_executor_roundtrip() {
        // Create client with functions
        let mut client = Client::new();
        let func = make_const_function(99.0);
        let hash = client.store_mut().add(func);

        // Create executor
        let mut executor = Executor::new();

        // Execute via client
        let result = client
            .execute_on(&mut executor, hash, &[])
            .expect("should execute");

        assert_eq!(result, Value::Number(99.0));
    }

    #[test]
    fn test_client_executor_with_dependencies() {
        let mut client = Client::new();

        // Add callee and caller
        let add_func = make_add_function();
        let add_hash = client.store_mut().add(add_func);

        let caller_func = make_caller_function(add_hash);
        let caller_hash = client.store_mut().add(caller_func);

        // Execute on fresh executor
        let mut executor = Executor::new();
        let result = client
            .execute_on(&mut executor, caller_hash, &[])
            .expect("should execute");

        assert_eq!(result, Value::Number(30.0));
    }

    #[test]
    fn test_executor_store_persistence() {
        let mut store = Store::new();
        let func = make_const_function(1.0);
        let hash = store.add(func);

        let mut executor = Executor::new();

        // First execution
        let result1 = executor
            .execute_from_store(hash, vec![], &store)
            .expect("first execution");
        assert_eq!(result1, Value::Number(1.0));

        // Second execution should work too (function persisted in executor)
        let result2 = executor
            .execute_from_store(hash, vec![], &Store::new())
            .expect("second execution");
        assert_eq!(result2, Value::Number(1.0));
    }

    #[test]
    fn test_isolated_vms() {
        // Verify that two executors have completely isolated state
        let mut store = Store::new();
        let func = make_const_function(42.0);
        let hash = store.add(func);

        // Load into executor 1
        let mut executor1 = Executor::new();
        executor1
            .execute_from_store(hash, vec![], &store)
            .expect("executor1");

        // Executor 2 should NOT have the function
        let executor2 = Executor::new();
        assert!(!executor2.store().contains(&hash));
    }

    #[test]
    fn test_execution_response_serialization() {
        // Test that responses can be serialized/deserialized
        let success = ExecutionResponse::Success(Value::Number(42.0));
        let json = serde_json::to_string(&success).expect("serialize");
        let parsed: ExecutionResponse = serde_json::from_str(&json).expect("deserialize");

        match parsed {
            ExecutionResponse::Success(Value::Number(n)) => assert!((n - 42.0).abs() < f64::EPSILON),
            _ => panic!("unexpected response type"),
        }
    }

    #[test]
    fn test_execution_request_serialization() {
        let mut store = Store::new();
        let func = make_const_function(1.0);
        let hash = store.add(func);

        let request = ExecutionRequest::new(hash, vec![Value::Bool(true)], &store)
            .expect("create request");

        let json = serde_json::to_string(&request).expect("serialize");
        let parsed: ExecutionRequest = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.function_hash, hash);
        assert_eq!(parsed.arguments.len(), 1);
    }
}

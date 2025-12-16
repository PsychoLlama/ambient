//! Built-in abilities for the Ambient VM.
//!
//! This module provides host handler implementations for abilities.
//! Ability IDs and descriptors are defined in `ambient-core` (for Exception)
//! and `ambient-runtime` (for Console, Time, Random, Async, Log).
//!
//! This module re-exports the ability ID modules for backward compatibility.

#![allow(clippy::type_complexity)] // Handler types are inherently complex

// Re-export format_value for backward compatibility
pub use crate::format::format_value;

use crate::value::{SuspendedAbility, Value};
use crate::vm::{Vm, VmError};

// ═══════════════════════════════════════════════════════════════════════════
// Re-export ability IDs from core and runtime crates
// ═══════════════════════════════════════════════════════════════════════════

/// Console ability - for printing to stdout/stderr.
pub mod console {
    pub use ambient_runtime::console::*;
}

/// Exception ability - for throwing and catching errors.
pub mod exception {
    pub use ambient_core::exception::*;
}

/// Time ability - for time-related operations.
pub mod time {
    pub use ambient_runtime::time::*;
}

/// Random ability - for random number generation.
pub mod random {
    pub use ambient_runtime::random::*;
}

/// Async ability - for concurrent execution of abilities.
pub mod async_ability {
    pub use ambient_runtime::async_ability::*;
}

/// Remote ability - for remote function execution.
pub mod remote {
    pub use ambient_runtime::remote::*;
}

// ═══════════════════════════════════════════════════════════════════════════
// Console Ability Implementation
// ═══════════════════════════════════════════════════════════════════════════

/// Configuration for the Console ability.
///
/// Handlers must be Send + Sync to allow the VM to be used across threads.
#[derive(Default)]
pub struct ConsoleConfig {
    /// Custom print handler. If None, uses stdout.
    pub print_handler: Option<Box<dyn Fn(&str) + Send + Sync>>,
    /// Custom eprint handler. If None, uses stderr.
    pub eprint_handler: Option<Box<dyn Fn(&str) + Send + Sync>>,
}

/// Register the Console ability handlers on a VM.
///
/// By default, this prints to stdout/stderr. Use `ConsoleConfig` to customize.
pub fn register_console(vm: &mut Vm, config: ConsoleConfig) {
    // Console.print - prints with newline
    let print_handler = config.print_handler;
    vm.register_host_handler(
        console::ABILITY_ID,
        console::METHOD_PRINT,
        Box::new(move |ability: &SuspendedAbility| {
            let message = format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
            if let Some(ref handler) = print_handler {
                handler(&message);
            } else {
                #[cfg(not(test))]
                {
                    #[allow(clippy::print_stdout)]
                    {
                        println!("{message}");
                    }
                }
            }
            Ok(Value::Unit)
        }),
    );

    // Console.println - same as print (both add newline)
    vm.register_host_handler(
        console::ABILITY_ID,
        console::METHOD_PRINTLN,
        Box::new(|ability: &SuspendedAbility| {
            let message = format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
            #[cfg(not(test))]
            {
                #[allow(clippy::print_stdout)]
                {
                    println!("{message}");
                }
            }
            let _ = message; // Suppress unused warning in test mode
            Ok(Value::Unit)
        }),
    );

    // Console.eprint - prints to stderr with newline
    let eprint_handler = config.eprint_handler;
    vm.register_host_handler(
        console::ABILITY_ID,
        console::METHOD_EPRINT,
        Box::new(move |ability: &SuspendedAbility| {
            let message = format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
            if let Some(ref handler) = eprint_handler {
                handler(&message);
            } else {
                #[cfg(not(test))]
                {
                    #[allow(clippy::print_stderr)]
                    {
                        eprintln!("{message}");
                    }
                }
            }
            Ok(Value::Unit)
        }),
    );
}

/// Register Console with a custom output collector (useful for testing).
///
/// Uses `Arc<Mutex<>>` for thread safety.
///
/// # Panics
///
/// Panics if the mutex is poisoned.
#[cfg(test)]
pub fn register_console_with_collector(
    vm: &mut Vm,
    output: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    let output_clone = output.clone();
    vm.register_host_handler(
        console::ABILITY_ID,
        console::METHOD_PRINT,
        Box::new(move |ability: &SuspendedAbility| {
            let message = format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
            output_clone.lock().expect("lock poisoned").push(message);
            Ok(Value::Unit)
        }),
    );

    let output_clone = output.clone();
    vm.register_host_handler(
        console::ABILITY_ID,
        console::METHOD_PRINTLN,
        Box::new(move |ability: &SuspendedAbility| {
            let message = format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
            output_clone
                .lock()
                .expect("lock poisoned")
                .push(format!("{message}\n"));
            Ok(Value::Unit)
        }),
    );

    vm.register_host_handler(
        console::ABILITY_ID,
        console::METHOD_EPRINT,
        Box::new(move |ability: &SuspendedAbility| {
            let message = format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
            output
                .lock()
                .expect("lock poisoned")
                .push(format!("[ERR] {message}"));
            Ok(Value::Unit)
        }),
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Exception Ability Implementation
// ═══════════════════════════════════════════════════════════════════════════

/// Error type for unhandled exceptions.
#[derive(Debug, Clone, PartialEq)]
pub struct UnhandledException {
    /// The error value that was thrown.
    pub error: Value,
}

impl std::fmt::Display for UnhandledException {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unhandled exception: {}", format_value(&self.error))
    }
}

impl std::error::Error for UnhandledException {}

/// Register a default Exception handler that returns `VmError`.
///
/// Note: Exception handling is typically done via bytecode handlers (try/catch),
/// not host handlers. This provides a fallback for unhandled exceptions.
pub fn register_exception_fallback(vm: &mut Vm) {
    vm.register_host_handler(
        exception::ABILITY_ID,
        exception::METHOD_THROW,
        Box::new(|_ability: &SuspendedAbility| {
            // Return an error - this stops execution
            // The actual error value is in ability.args[0] but we return UnhandledAbility
            Err(VmError::UnhandledAbility {
                ability_id: exception::ABILITY_ID,
                method_id: exception::METHOD_THROW,
            })
        }),
    );
}

/// Log ability - for structured logging with levels.
pub mod log {
    pub use ambient_runtime::log::*;
}

// ═══════════════════════════════════════════════════════════════════════════
// Time Ability Implementation
// ═══════════════════════════════════════════════════════════════════════════

/// Register the Time ability handlers on a VM.
///
/// Provides `now()` for getting current timestamp and `wait(ms)` for sleeping.
pub fn register_time(vm: &mut Vm) {
    // Time.now() -> returns current timestamp in milliseconds since Unix epoch
    vm.register_host_handler(
        time::ABILITY_ID,
        time::METHOD_NOW,
        Box::new(|_ability: &SuspendedAbility| {
            use std::time::{SystemTime, UNIX_EPOCH};
            // Precision loss is acceptable for timestamps (won't exceed 52 bits for centuries)
            #[allow(clippy::cast_precision_loss)]
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as f64)
                .unwrap_or(0.0);
            Ok(Value::Number(now))
        }),
    );

    // Time.wait(duration) -> sleeps for the given number of milliseconds
    vm.register_host_handler(
        time::ABILITY_ID,
        time::METHOD_WAIT,
        Box::new(|ability: &SuspendedAbility| {
            if let Some(Value::Number(ms)) = ability.args.first() {
                // Negative durations are clamped to 0
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let ms_u64 = if *ms < 0.0 { 0 } else { *ms as u64 };
                let duration = std::time::Duration::from_millis(ms_u64);
                std::thread::sleep(duration);
            }
            Ok(Value::Unit)
        }),
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Random Ability Implementation
// ═══════════════════════════════════════════════════════════════════════════

/// Register the Random ability handlers on a VM.
///
/// Provides `seed()` for random 0.0-1.0 and `in_range(range)` for random in range.
pub fn register_random(vm: &mut Vm) {
    use std::sync::atomic::{AtomicU64, Ordering};

    // Simple xorshift64 PRNG state - good enough for most purposes
    // Seeded from system time on first use
    static SEED: AtomicU64 = AtomicU64::new(0);

    fn next_random() -> f64 {
        // Initialize seed if needed (using system time)
        let mut state = SEED.load(Ordering::Relaxed);
        if state == 0 {
            use std::time::{SystemTime, UNIX_EPOCH};
            // Truncation is intentional - we only need 64 bits of entropy
            #[allow(clippy::cast_possible_truncation)]
            let time_seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x853c_49e6_748f_ea9b);
            state = time_seed;
            if state == 0 {
                state = 0x853c_49e6_748f_ea9b; // fallback seed
            }
        }

        // xorshift64
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        SEED.store(state, Ordering::Relaxed);

        // Convert to 0.0-1.0 range
        // Cast precision loss is acceptable for random number generation
        #[allow(clippy::cast_precision_loss)]
        let result = (state as f64) / (u64::MAX as f64);
        result
    }

    // Random.seed() -> returns random number between 0.0 and 1.0
    vm.register_host_handler(
        random::ABILITY_ID,
        random::METHOD_SEED,
        Box::new(|_ability: &SuspendedAbility| Ok(Value::Number(next_random()))),
    );

    // Random.in_range(range) -> returns random number in the given range
    // Range is expected as a record { start: number, end: number }
    vm.register_host_handler(
        random::ABILITY_ID,
        random::METHOD_IN_RANGE,
        Box::new(|ability: &SuspendedAbility| {
            if let Some(Value::Record(fields)) = ability.args.first() {
                let start = fields
                    .get(&std::sync::Arc::from("start"))
                    .and_then(|v| match v {
                        Value::Number(n) => Some(*n),
                        _ => None,
                    })
                    .unwrap_or(0.0);
                let end = fields
                    .get(&std::sync::Arc::from("end"))
                    .and_then(|v| match v {
                        Value::Number(n) => Some(*n),
                        _ => None,
                    })
                    .unwrap_or(1.0);

                let random = next_random();
                let result = start + random * (end - start);
                Ok(Value::Number(result))
            } else {
                // If not a record, treat as single number for upper bound
                let upper = ability
                    .args
                    .first()
                    .and_then(|v| match v {
                        Value::Number(n) => Some(*n),
                        _ => None,
                    })
                    .unwrap_or(1.0);
                Ok(Value::Number(next_random() * upper))
            }
        }),
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Log Ability Implementation
// ═══════════════════════════════════════════════════════════════════════════

/// Configuration for the Log ability.
#[derive(Default)]
pub struct LogConfig {
    /// Minimum log level to output (0 = debug, 1 = info, 2 = warn, 3 = error)
    pub min_level: u8,
    /// Custom log handler. If None, uses default formatting to stdout.
    pub handler: Option<Box<dyn Fn(&str, &str) + Send + Sync>>,
}

/// Register the Log ability handlers on a VM.
///
/// Provides structured logging with debug, info, warn, and error levels.
pub fn register_log(vm: &mut Vm, config: LogConfig) {
    let min_level = config.min_level;
    let handler = std::sync::Arc::new(config.handler);

    // Helper to create log handlers
    macro_rules! log_handler {
        ($level:expr, $prefix:expr) => {{
            let handler = handler.clone();
            Box::new(move |ability: &SuspendedAbility| {
                if $level >= min_level {
                    let message =
                        format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
                    if let Some(ref h) = *handler {
                        h($prefix, &message);
                    } else {
                        #[cfg(not(test))]
                        {
                            #[allow(clippy::print_stdout)]
                            {
                                println!("[{}] {}", $prefix, message);
                            }
                        }
                    }
                }
                Ok(Value::Unit)
            })
        }};
    }

    vm.register_host_handler(log::ABILITY_ID, log::METHOD_DEBUG, log_handler!(0, "DEBUG"));
    vm.register_host_handler(log::ABILITY_ID, log::METHOD_INFO, log_handler!(1, "INFO"));
    vm.register_host_handler(log::ABILITY_ID, log::METHOD_WARN, log_handler!(2, "WARN"));
    vm.register_host_handler(log::ABILITY_ID, log::METHOD_ERROR, log_handler!(3, "ERROR"));
}

// ═══════════════════════════════════════════════════════════════════════════
// Remote Ability Implementation
// ═══════════════════════════════════════════════════════════════════════════

use std::sync::{Arc, Mutex};

use tokio::runtime::Handle as RuntimeHandle;

use crate::bytecode::CompiledFunction;
use crate::protocol::{read_message, write_message, ErrorKind, Message};
use crate::remote_state::{ConnectionId, RemoteState};
use crate::store::{PortableFunction, Store};

/// Configuration for the Remote ability.
pub struct RemoteConfig {
    /// Tokio runtime handle for async operations.
    pub runtime: RuntimeHandle,
    /// Store for function lookup (needed to send dependencies).
    pub store: Arc<Mutex<Store>>,
}

/// Register the Remote ability handlers on a VM.
///
/// Provides network operations for remote function execution:
/// - `listen(address)` - Bind TCP listener
/// - `accept(listener)` - Accept connection
/// - `connect(address)` - Connect to server
/// - `call(conn, thunk)` - Send thunk for remote execution
/// - `serve(conn)` - Wait for and execute one remote call
/// - `close(conn)` - Close connection
#[allow(clippy::too_many_lines)]
pub fn register_remote(vm: &mut Vm, config: RemoteConfig) {
    let state = Arc::new(Mutex::new(RemoteState::new(config.runtime, config.store)));

    // Remote.listen(address: string) -> Listener (number handle)
    let state_clone = Arc::clone(&state);
    vm.register_host_handler(
        remote::ABILITY_ID,
        remote::METHOD_LISTEN,
        Box::new(move |ability: &SuspendedAbility| {
            let addr = extract_string(&ability.args)?;
            let mut state = state_clone.lock().map_err(|_| VmError::LockPoisoned)?;
            let id = state
                .listen(&addr)
                .map_err(|e| VmError::IoError(e.to_string()))?;
            // Handle IDs are small integers; precision loss only happens after 2^53
            #[allow(clippy::cast_precision_loss)]
            Ok(Value::Number(id as f64))
        }),
    );

    // Remote.accept(listener: number) -> Connection (number handle)
    let state_clone = Arc::clone(&state);
    vm.register_host_handler(
        remote::ABILITY_ID,
        remote::METHOD_ACCEPT,
        Box::new(move |ability: &SuspendedAbility| {
            // Handle IDs from Ambient are always non-negative integers
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let listener_id = extract_number(&ability.args)? as u64;
            let mut state = state_clone.lock().map_err(|_| VmError::LockPoisoned)?;
            let id = state
                .accept(listener_id)
                .map_err(|e| VmError::IoError(e.to_string()))?;
            #[allow(clippy::cast_precision_loss)]
            Ok(Value::Number(id as f64))
        }),
    );

    // Remote.connect(address: string) -> Connection (number handle)
    let state_clone = Arc::clone(&state);
    vm.register_host_handler(
        remote::ABILITY_ID,
        remote::METHOD_CONNECT,
        Box::new(move |ability: &SuspendedAbility| {
            let addr = extract_string(&ability.args)?;
            let mut state = state_clone.lock().map_err(|_| VmError::LockPoisoned)?;
            let id = state
                .connect(&addr)
                .map_err(|e| VmError::IoError(e.to_string()))?;
            #[allow(clippy::cast_precision_loss)]
            Ok(Value::Number(id as f64))
        }),
    );

    // Remote.call(conn: number, thunk: closure) -> value
    let state_clone = Arc::clone(&state);
    vm.register_host_handler(
        remote::ABILITY_ID,
        remote::METHOD_CALL,
        Box::new(move |ability: &SuspendedAbility| {
            if ability.args.len() < 2 {
                return Err(VmError::TypeErrorOwned {
                    expected: "2 arguments".to_string(),
                    got: format!("{} arguments", ability.args.len()),
                });
            }

            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let conn_id = match &ability.args[0] {
                Value::Number(n) => *n as u64,
                other => {
                    return Err(VmError::TypeErrorOwned {
                        expected: "number".to_string(),
                        got: other.type_name().to_string(),
                    })
                }
            };

            let closure = match &ability.args[1] {
                Value::Closure(c) => Arc::clone(c),
                other => {
                    return Err(VmError::TypeErrorOwned {
                        expected: "closure".to_string(),
                        got: other.type_name().to_string(),
                    })
                }
            };

            let mut state = state_clone.lock().map_err(|_| VmError::LockPoisoned)?;
            handle_remote_call(&mut state, conn_id, &closure)
        }),
    );

    // Remote.serve(conn: number) -> value
    let state_clone = Arc::clone(&state);
    vm.register_host_handler(
        remote::ABILITY_ID,
        remote::METHOD_SERVE,
        Box::new(move |ability: &SuspendedAbility| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let conn_id = extract_number(&ability.args)? as u64;
            let mut state = state_clone.lock().map_err(|_| VmError::LockPoisoned)?;
            handle_remote_serve(&mut state, conn_id)
        }),
    );

    // Remote.close(conn: number) -> ()
    let state_clone = Arc::clone(&state);
    vm.register_host_handler(
        remote::ABILITY_ID,
        remote::METHOD_CLOSE,
        Box::new(move |ability: &SuspendedAbility| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let conn_id = extract_number(&ability.args)? as u64;
            let mut state = state_clone.lock().map_err(|_| VmError::LockPoisoned)?;
            state
                .close(conn_id)
                .map_err(|e| VmError::IoError(e.to_string()))?;
            Ok(Value::Unit)
        }),
    );
}

/// Extract a string from the first argument.
fn extract_string(args: &[Value]) -> Result<String, VmError> {
    match args.first() {
        Some(Value::String(s)) => Ok(s.to_string()),
        Some(other) => Err(VmError::TypeErrorOwned {
            expected: "string".to_string(),
            got: other.type_name().to_string(),
        }),
        None => Err(VmError::TypeErrorOwned {
            expected: "string".to_string(),
            got: "no argument".to_string(),
        }),
    }
}

/// Extract a number from the first argument.
fn extract_number(args: &[Value]) -> Result<f64, VmError> {
    match args.first() {
        Some(Value::Number(n)) => Ok(*n),
        Some(other) => Err(VmError::TypeErrorOwned {
            expected: "number".to_string(),
            got: other.type_name().to_string(),
        }),
        None => Err(VmError::TypeErrorOwned {
            expected: "number".to_string(),
            got: "no argument".to_string(),
        }),
    }
}

/// Handle Remote.call - send closure for remote execution.
fn handle_remote_call(
    state: &mut RemoteState,
    conn_id: ConnectionId,
    closure: &crate::value::Closure,
) -> Result<Value, VmError> {
    // Get function hash and captured environment
    let func_hash = closure.function_hash;
    let captures = closure.environment.clone();

    // For the PoC, since we can't write zero-arg lambdas like `() => expr`,
    // we use lambdas with a dummy parameter like `(x) => expr`. We pass
    // the captured values as the args (one value per parameter).
    // This is a workaround until we support zero-arg lambdas.
    let args = captures.clone();

    // Collect function and dependencies from client's store
    let store = state.store().lock().map_err(|_| VmError::LockPoisoned)?;
    let subset = store.extract_with_dependencies(&func_hash);
    let portable_functions: Vec<PortableFunction> = subset
        .hashes()
        .iter()
        .filter_map(|h| subset.get(h).map(|f| PortableFunction::from(f.as_ref())))
        .collect();
    drop(store);

    // Get runtime before getting mutable borrow
    let runtime = state.runtime().clone();

    // Get the connection
    let conn = state
        .get_connection_mut(conn_id)
        .ok_or_else(|| VmError::IoError(format!("invalid connection ID: {conn_id}")))?;

    // Split the stream for read/write
    let stream = &mut conn.stream;

    runtime.block_on(async {
        // Send Execute message with closure's captured values
        let (mut reader, mut writer) = stream.split();
        write_message(
            &mut writer,
            &Message::execute_closure(func_hash, args, captures),
        )
        .await
        .map_err(|e| VmError::IoError(e.to_string()))?;

        // Handle response loop
        loop {
            let response = read_message(&mut reader)
                .await
                .map_err(|e| VmError::IoError(e.to_string()))?
                .ok_or_else(|| VmError::IoError("connection closed".to_string()))?;

            match response {
                Message::Result { value } => return Ok(value),
                Message::Error { error } => {
                    return Err(VmError::IoError(format!("remote error: {}", error.message)))
                }
                Message::NeedDeps { hashes } => {
                    // Provide requested functions
                    let functions: Vec<_> = hashes
                        .iter()
                        .filter_map(|h| portable_functions.iter().find(|f| f.hash == *h).cloned())
                        .collect();
                    write_message(&mut writer, &Message::Provide { functions })
                        .await
                        .map_err(|e| VmError::IoError(e.to_string()))?;
                }
                _ => {
                    return Err(VmError::IoError(
                        "unexpected message from server".to_string(),
                    ))
                }
            }
        }
    })
}

/// Handle Remote.serve - wait for and execute one remote call.
fn handle_remote_serve(state: &mut RemoteState, conn_id: ConnectionId) -> Result<Value, VmError> {
    // Get runtime before getting mutable borrow
    let runtime = state.runtime().clone();

    let conn = state
        .get_connection_mut(conn_id)
        .ok_or_else(|| VmError::IoError(format!("invalid connection ID: {conn_id}")))?;

    let executor = conn
        .executor
        .as_mut()
        .ok_or_else(|| VmError::IoError("not a server connection".to_string()))?;

    let stream = &mut conn.stream;

    runtime.block_on(async {
        let (mut reader, mut writer) = stream.split();

        // Track the pending execute request: (hash, args, captures)
        let mut pending_execute: Option<(blake3::Hash, Vec<Value>, Vec<Value>)> = None;

        loop {
            // If we have a pending execute, try to run it
            if let Some((hash, ref args, ref captures)) = pending_execute {
                // Check if we now have the function
                if executor.store().contains(&hash) {
                    // Check for missing dependencies
                    let missing = executor.store().missing_dependencies(&hash);
                    if missing.is_empty() {
                        // Execute the closure with its captured environment
                        let result =
                            executor
                                .vm_mut()
                                .call_closure(&hash, args.clone(), captures.clone());
                        match result {
                            Ok(value) => {
                                write_message(&mut writer, &Message::result(value.clone()))
                                    .await
                                    .map_err(|e| VmError::IoError(e.to_string()))?;
                                return Ok(value);
                            }
                            Err(e) => {
                                write_message(
                                    &mut writer,
                                    &Message::error(ErrorKind::RuntimeException, e.to_string()),
                                )
                                .await
                                .map_err(|e| VmError::IoError(e.to_string()))?;
                                return Err(e);
                            }
                        }
                    }
                    // Missing dependencies - request them and wait for Provide
                    write_message(&mut writer, &Message::need_deps(&missing))
                        .await
                        .map_err(|e| VmError::IoError(e.to_string()))?;
                } else {
                    // Still don't have the function, request it
                    write_message(&mut writer, &Message::need_deps(&[hash]))
                        .await
                        .map_err(|e| VmError::IoError(e.to_string()))?;
                }
            }

            // Read next message
            let msg = read_message(&mut reader)
                .await
                .map_err(|e| VmError::IoError(e.to_string()))?
                .ok_or_else(|| VmError::IoError("connection closed".to_string()))?;

            match msg {
                Message::Execute {
                    function,
                    args,
                    captures,
                } => {
                    // Parse the function hash
                    let hash = parse_hash(&function)
                        .map_err(|e| VmError::IoError(format!("invalid hash: {e}")))?;

                    // Store as pending with captures
                    pending_execute = Some((hash, args, captures));
                    // Loop will try to execute on next iteration
                }
                Message::Provide { functions } => {
                    // Load provided functions into executor
                    for pf in functions {
                        let func = CompiledFunction::try_from(pf)
                            .map_err(|e| VmError::IoError(format!("invalid function: {e}")))?;
                        executor.store_mut().add(func.clone());
                        executor.vm_mut().load_function(func);
                    }
                    // Loop will retry pending execute
                }
                _ => {
                    // Ignore other messages
                }
            }
        }
    })
}

/// Parse a hex-encoded hash string.
fn parse_hash(hex_str: &str) -> Result<blake3::Hash, String> {
    let bytes = hex::decode(hex_str).map_err(|e| format!("invalid hex: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "invalid hash length: expected 32 bytes, got {}",
            bytes.len()
        ));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(blake3::Hash::from_bytes(arr))
}

// ═══════════════════════════════════════════════════════════════════════════
// Convenience function to register all standard abilities
// ═══════════════════════════════════════════════════════════════════════════

/// Register all standard ability handlers on a VM.
///
/// This includes Console, Exception (fallback), Time, Random, and Log.
/// Note: Remote is NOT included here as it requires a runtime handle.
pub fn register_all_standard_abilities(vm: &mut Vm) {
    register_console(vm, ConsoleConfig::default());
    register_exception_fallback(vm);
    register_time(vm);
    register_random(vm);
    register_log(vm, LogConfig::default());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::{BytecodeBuilder, Opcode};
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_console_print_with_collector() {
        let output: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::string("Hello, World!"));
        builder.emit_suspend(console::ABILITY_ID, console::METHOD_PRINT, 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_console_with_collector(&mut vm, output.clone());

        let result = vm.call(&hash, vec![]);
        assert_eq!(result, Ok(Value::Unit));
        assert_eq!(*output.lock().expect("lock"), vec!["Hello, World!"]);
    }

    #[test]
    fn test_console_println_with_collector() {
        let output: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Number(42.0));
        builder.emit_suspend(console::ABILITY_ID, console::METHOD_PRINTLN, 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_console_with_collector(&mut vm, output.clone());

        let result = vm.call(&hash, vec![]);
        assert_eq!(result, Ok(Value::Unit));
        assert_eq!(*output.lock().expect("lock"), vec!["42\n"]);
    }

    #[test]
    fn test_format_value() {
        assert_eq!(format_value(&Value::Unit), "()");
        assert_eq!(format_value(&Value::Bool(true)), "true");
        assert_eq!(format_value(&Value::Number(42.0)), "42");
        assert_eq!(format_value(&Value::Number(3.14)), "3.14");
        assert_eq!(format_value(&Value::string("hello")), "hello");
        assert_eq!(
            format_value(&Value::tuple(vec![Value::Number(1.0), Value::Number(2.0)])),
            "(1, 2)"
        );
    }

    #[test]
    fn test_multiple_prints() {
        let output: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let mut builder = BytecodeBuilder::new();

        // First print
        builder.emit_const(Value::string("one"));
        builder.emit_suspend(console::ABILITY_ID, console::METHOD_PRINT, 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Pop);

        // Second print
        builder.emit_const(Value::string("two"));
        builder.emit_suspend(console::ABILITY_ID, console::METHOD_PRINT, 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Pop);

        // Third print
        builder.emit_const(Value::string("three"));
        builder.emit_suspend(console::ABILITY_ID, console::METHOD_PRINT, 1);
        builder.emit(Opcode::Perform);

        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_console_with_collector(&mut vm, output.clone());

        let result = vm.call(&hash, vec![]);
        assert_eq!(result, Ok(Value::Unit));
        assert_eq!(*output.lock().expect("lock"), vec!["one", "two", "three"]);
    }

    // =========================================================================
    // Time Ability Tests
    // =========================================================================

    #[test]
    fn test_time_now() {
        let mut builder = BytecodeBuilder::new();
        builder.emit_suspend(time::ABILITY_ID, time::METHOD_NOW, 0);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_time(&mut vm);

        let result = vm.call(&hash, vec![]);
        assert!(result.is_ok());
        if let Ok(Value::Number(timestamp)) = result {
            // Timestamp should be a positive number representing milliseconds since epoch
            assert!(timestamp > 0.0);
            // Should be after year 2020 (1577836800000 ms)
            assert!(timestamp > 1_577_836_800_000.0);
        } else {
            panic!("Expected Number, got {:?}", result);
        }
    }

    #[test]
    fn test_time_wait() {
        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Number(10.0)); // Wait 10ms
        builder.emit_suspend(time::ABILITY_ID, time::METHOD_WAIT, 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_time(&mut vm);

        let start = std::time::Instant::now();
        let result = vm.call(&hash, vec![]);
        let elapsed = start.elapsed();

        assert_eq!(result, Ok(Value::Unit));
        // Should have waited at least 10ms (with some tolerance)
        assert!(elapsed.as_millis() >= 9);
    }

    // =========================================================================
    // Random Ability Tests
    // =========================================================================

    #[test]
    fn test_random_seed() {
        let mut builder = BytecodeBuilder::new();
        builder.emit_suspend(random::ABILITY_ID, random::METHOD_SEED, 0);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_random(&mut vm);

        let result = vm.call(&hash, vec![]);
        assert!(result.is_ok());
        if let Ok(Value::Number(n)) = result {
            assert!(n >= 0.0 && n <= 1.0, "Random should be in [0, 1]: {n}");
        } else {
            panic!("Expected Number, got {:?}", result);
        }
    }

    #[test]
    fn test_random_in_range() {
        let mut builder = BytecodeBuilder::new();
        // Create a range record { start: 10, end: 20 }
        builder.emit_const(Value::string("start"));
        builder.emit_const(Value::Number(10.0));
        builder.emit_const(Value::string("end"));
        builder.emit_const(Value::Number(20.0));
        builder.emit_u8(Opcode::MakeRecord, 2);
        builder.emit_suspend(random::ABILITY_ID, random::METHOD_IN_RANGE, 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_random(&mut vm);

        let result = vm.call(&hash, vec![]);
        assert!(result.is_ok());
        if let Ok(Value::Number(n)) = result {
            assert!(n >= 10.0 && n <= 20.0, "Random should be in [10, 20]: {n}");
        } else {
            panic!("Expected Number, got {:?}", result);
        }
    }

    #[test]
    fn test_random_produces_different_values() {
        let mut builder = BytecodeBuilder::new();
        // Generate two random numbers and return them as a tuple
        builder.emit_suspend(random::ABILITY_ID, random::METHOD_SEED, 0);
        builder.emit(Opcode::Perform);
        builder.emit_suspend(random::ABILITY_ID, random::METHOD_SEED, 0);
        builder.emit(Opcode::Perform);
        builder.emit_u8(Opcode::MakeTuple, 2);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_random(&mut vm);

        let result = vm.call(&hash, vec![]);
        assert!(result.is_ok());
        if let Ok(Value::Tuple(elements)) = result {
            assert_eq!(elements.len(), 2);
            // Two consecutive random numbers should (usually) be different
            // This could theoretically fail, but with 64-bit state, probability is negligible
            if let (Value::Number(a), Value::Number(b)) = (&elements[0], &elements[1]) {
                assert_ne!(a, b, "Two random numbers should be different");
            }
        } else {
            panic!("Expected Tuple, got {:?}", result);
        }
    }

    // =========================================================================
    // Log Ability Tests
    // =========================================================================

    #[test]
    fn test_log_info() {
        let output: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let output_clone = output.clone();

        let config = LogConfig {
            min_level: 0,
            handler: Some(Box::new(move |level: &str, msg: &str| {
                output_clone
                    .lock()
                    .expect("lock")
                    .push((level.to_string(), msg.to_string()));
            })),
        };

        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::string("test message"));
        builder.emit_suspend(log::ABILITY_ID, log::METHOD_INFO, 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_log(&mut vm, config);

        let result = vm.call(&hash, vec![]);
        assert_eq!(result, Ok(Value::Unit));

        let logs = output.lock().expect("lock");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0], ("INFO".to_string(), "test message".to_string()));
    }

    #[test]
    fn test_log_min_level_filters() {
        let output: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let output_clone = output.clone();

        // Set min level to WARN (2), so DEBUG and INFO should be filtered
        let config = LogConfig {
            min_level: 2,
            handler: Some(Box::new(move |level: &str, msg: &str| {
                output_clone
                    .lock()
                    .expect("lock")
                    .push((level.to_string(), msg.to_string()));
            })),
        };

        let mut builder = BytecodeBuilder::new();

        // Log debug (should be filtered)
        builder.emit_const(Value::string("debug msg"));
        builder.emit_suspend(log::ABILITY_ID, log::METHOD_DEBUG, 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Pop);

        // Log warn (should pass)
        builder.emit_const(Value::string("warn msg"));
        builder.emit_suspend(log::ABILITY_ID, log::METHOD_WARN, 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Pop);

        // Log error (should pass)
        builder.emit_const(Value::string("error msg"));
        builder.emit_suspend(log::ABILITY_ID, log::METHOD_ERROR, 1);
        builder.emit(Opcode::Perform);

        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_log(&mut vm, config);

        let result = vm.call(&hash, vec![]);
        assert_eq!(result, Ok(Value::Unit));

        let logs = output.lock().expect("lock");
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0], ("WARN".to_string(), "warn msg".to_string()));
        assert_eq!(logs[1], ("ERROR".to_string(), "error msg".to_string()));
    }

    #[test]
    fn test_register_all_standard_abilities() {
        let mut vm = Vm::new();
        register_all_standard_abilities(&mut vm);

        // Verify we can call Time.now
        let mut builder = BytecodeBuilder::new();
        builder.emit_suspend(time::ABILITY_ID, time::METHOD_NOW, 0);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;
        vm.load_function(func);

        let result = vm.call(&hash, vec![]);
        assert!(result.is_ok());
    }
}

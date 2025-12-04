//! Built-in abilities for the Ambient VM.
//!
//! This module defines well-known ability IDs and provides host handler
//! implementations for core abilities like Console and Exception.

#![allow(clippy::type_complexity)] // Handler types are inherently complex

use crate::value::{SuspendedAbility, Value};
use crate::vm::{Vm, VmError};

// ═══════════════════════════════════════════════════════════════════════════
// Ability IDs
// ═══════════════════════════════════════════════════════════════════════════

/// Console ability - for printing to stdout/stderr.
pub mod console {
    /// Ability ID for Console.
    pub const ABILITY_ID: u16 = 0x0001;

    /// Method: print a message to stdout.
    pub const METHOD_PRINT: u16 = 0x0000;

    /// Method: print a message to stderr.
    pub const METHOD_EPRINT: u16 = 0x0001;

    /// Method: print with newline.
    pub const METHOD_PRINTLN: u16 = 0x0002;
}

/// Exception ability - for throwing and catching errors.
pub mod exception {
    /// Ability ID for Exception.
    pub const ABILITY_ID: u16 = 0x0002;

    /// Method: throw an error (never returns normally).
    pub const METHOD_THROW: u16 = 0x0000;
}

/// Time ability - for time-related operations.
pub mod time {
    /// Ability ID for Time.
    pub const ABILITY_ID: u16 = 0x0003;

    /// Method: get current timestamp in milliseconds.
    pub const METHOD_NOW: u16 = 0x0000;

    /// Method: wait for a duration in milliseconds.
    pub const METHOD_WAIT: u16 = 0x0001;
}

/// Random ability - for random number generation.
pub mod random {
    /// Ability ID for Random.
    pub const ABILITY_ID: u16 = 0x0004;

    /// Method: get a random number between 0.0 and 1.0.
    pub const METHOD_SEED: u16 = 0x0000;

    /// Method: get a random number in a range.
    pub const METHOD_IN_RANGE: u16 = 0x0001;
}

/// Async ability - for concurrent execution of abilities.
pub mod async_ability {
    /// Ability ID for Async.
    pub const ABILITY_ID: u16 = 0x0005;

    /// Method: wait for all operations to complete.
    /// Takes a tuple of suspended abilities, returns a tuple of results.
    pub const METHOD_ALL: u16 = 0x0000;

    /// Method: wait for first operation to complete, cancel others.
    /// Takes a tuple of suspended abilities, returns the first result.
    pub const METHOD_RACE: u16 = 0x0001;
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
    // Console.print
    let print_handler = config.print_handler;
    vm.register_host_handler(
        console::ABILITY_ID,
        console::METHOD_PRINT,
        Box::new(move |ability: &SuspendedAbility| {
            let message = format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
            if let Some(ref handler) = print_handler {
                handler(&message);
            } else {
                // In production, we'd use print! but tests don't want stdout
                #[cfg(not(test))]
                {
                    #[allow(clippy::print_stdout)]
                    {
                        print!("{message}");
                    }
                }
            }
            Ok(Value::Unit)
        }),
    );

    // Console.println
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

    // Console.eprint
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
                        eprint!("{message}");
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
            output_clone.lock().expect("lock poisoned").push(format!("{message}\n"));
            Ok(Value::Unit)
        }),
    );

    vm.register_host_handler(
        console::ABILITY_ID,
        console::METHOD_EPRINT,
        Box::new(move |ability: &SuspendedAbility| {
            let message = format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
            output.lock().expect("lock poisoned").push(format!("[ERR] {message}"));
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

// ═══════════════════════════════════════════════════════════════════════════
// Helper Functions
// ═══════════════════════════════════════════════════════════════════════════

/// Format a value as a string for display.
#[must_use]
pub fn format_value(value: &Value) -> String {
    match value {
        Value::Unit => "()".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            if n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{n:.0}")
            } else {
                n.to_string()
            }
        }
        Value::String(s) => s.to_string(),
        Value::Tuple(elements) => {
            let parts: Vec<String> = elements.iter().map(format_value).collect();
            let joined = parts.join(", ");
            format!("({joined})")
        }
        Value::Record(fields) => {
            let mut parts: Vec<String> = fields
                .iter()
                .map(|(k, v)| {
                    let v_str = format_value(v);
                    format!("{k}: {v_str}")
                })
                .collect();
            parts.sort(); // Consistent ordering
            let joined = parts.join(", ");
            format!("{{ {joined} }}")
        }
        Value::FunctionRef(hash) => {
            let hash_str = &hash.to_string()[..8];
            format!("<fn {hash_str}>")
        }
        Value::SuspendedAbility(ability) => {
            let ability_id = ability.ability_id;
            let method_id = ability.method_id;
            let arg_count = ability.args.len();
            format!("<ability {ability_id}:{method_id} with {arg_count} args>")
        }
        Value::Continuation(_) => "<continuation>".to_string(),
        Value::Closure(closure) => {
            let hash_str = &closure.function_hash.to_string()[..8];
            let capture_count = closure.environment.len();
            format!("<closure {hash_str} [{capture_count} captures]>")
        }
        Value::Handler(handler) => {
            let ability_id = handler.ability_id;
            let method_count = handler.methods.len();
            format!("<handler #{ability_id} [{method_count} methods]>")
        }
    }
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
}

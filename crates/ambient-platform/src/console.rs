//! Console ability - for printing to stdout/stderr.

#![allow(clippy::type_complexity)] // Handler types are inherently complex

use ambient_ability::{format_value, SuspendedAbility, Value, VmError};
use ambient_engine::ability_resolver::AbilityInterface;
use ambient_engine::vm::Vm;

use crate::require;

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
/// By default, this prints to stdout/stderr. Use [`ConsoleConfig`] to
/// customize.
///
/// # Panics
///
/// Panics if the resolved interface is missing an expected method — the
/// bindings interface and this handler set have drifted.
pub fn register_console(vm: &mut Vm, ability: &AbilityInterface, config: ConsoleConfig) {
    // Console.print - prints with newline
    let print_handler = config.print_handler;
    vm.register_host_handler(
        ability.id,
        require(ability, "print"),
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
        ability.id,
        require(ability, "println"),
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
        ability.id,
        require(ability, "eprint"),
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
/// Uses `Arc<Mutex<>>` for thread safety. A poisoned collector mutex
/// surfaces as a fatal `VmError::LockPoisoned`.
///
/// # Panics
///
/// Panics if the resolved interface is missing an expected method — the
/// bindings interface and this handler set have drifted.
pub fn register_console_with_collector(
    vm: &mut Vm,
    ability: &AbilityInterface,
    output: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    let output_clone = output.clone();
    vm.register_host_handler(
        ability.id,
        require(ability, "print"),
        Box::new(move |ability: &SuspendedAbility| {
            let message = format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
            output_clone
                .lock()
                .map_err(|_| VmError::LockPoisoned)?
                .push(message);
            Ok(Value::Unit)
        }),
    );

    let output_clone = output.clone();
    vm.register_host_handler(
        ability.id,
        require(ability, "println"),
        Box::new(move |ability: &SuspendedAbility| {
            let message = format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
            output_clone
                .lock()
                .map_err(|_| VmError::LockPoisoned)?
                .push(format!("{message}\n"));
            Ok(Value::Unit)
        }),
    );

    vm.register_host_handler(
        ability.id,
        require(ability, "eprint"),
        Box::new(move |ability: &SuspendedAbility| {
            let message = format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
            output
                .lock()
                .map_err(|_| VmError::LockPoisoned)?
                .push(format!("[ERR] {message}"));
            Ok(Value::Unit)
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_interface;
    use ambient_engine::bytecode::{BytecodeBuilder, Opcode};
    use std::sync::{Arc, Mutex};

    fn console_interface() -> AbilityInterface {
        test_interface("Console", 1, &["print", "eprint", "println"])
    }

    #[test]
    fn test_console_print_with_collector() {
        let ability = console_interface();
        let output: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::string("Hello, World!"));
        builder.emit_suspend(ability.id, require(&ability, "print"), 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_console_with_collector(&mut vm, &ability, output.clone());

        let result = vm.call(&hash, vec![]);
        assert_eq!(result, Ok(Value::Unit));
        assert_eq!(*output.lock().expect("lock"), vec!["Hello, World!"]);
    }

    #[test]
    fn test_console_println_with_collector() {
        let ability = console_interface();
        let output: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Number(42.0));
        builder.emit_suspend(ability.id, require(&ability, "println"), 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_console_with_collector(&mut vm, &ability, output.clone());

        let result = vm.call(&hash, vec![]);
        assert_eq!(result, Ok(Value::Unit));
        assert_eq!(*output.lock().expect("lock"), vec!["42\n"]);
    }

    #[test]
    fn test_multiple_prints() {
        let ability = console_interface();
        let output: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let mut builder = BytecodeBuilder::new();
        for message in ["one", "two", "three"] {
            builder.emit_const(Value::string(message));
            builder.emit_suspend(ability.id, require(&ability, "print"), 1);
            builder.emit(Opcode::Perform);
            if message != "three" {
                builder.emit(Opcode::Pop);
            }
        }
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_console_with_collector(&mut vm, &ability, output.clone());

        let result = vm.call(&hash, vec![]);
        assert_eq!(result, Ok(Value::Unit));
        assert_eq!(*output.lock().expect("lock"), vec!["one", "two", "three"]);
    }
}

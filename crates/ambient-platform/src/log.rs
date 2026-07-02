//! Log ability - for structured logging with levels.

#![allow(clippy::type_complexity)] // Handler types are inherently complex

use ambient_ability::{format_value, SuspendedAbility, Value};
use ambient_engine::ability_resolver::AbilityInterface;
use ambient_engine::vm::Vm;

use crate::require;

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
///
/// # Panics
///
/// Panics if the resolved interface is missing an expected method — the
/// bindings interface and this handler set have drifted.
pub fn register_log(vm: &mut Vm, ability: &AbilityInterface, config: LogConfig) {
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

    vm.register_host_handler(
        ability.id,
        require(ability, "debug"),
        log_handler!(0, "DEBUG"),
    );
    vm.register_host_handler(
        ability.id,
        require(ability, "info"),
        log_handler!(1, "INFO"),
    );
    vm.register_host_handler(
        ability.id,
        require(ability, "warn"),
        log_handler!(2, "WARN"),
    );
    vm.register_host_handler(
        ability.id,
        require(ability, "error"),
        log_handler!(3, "ERROR"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_interface;
    use ambient_engine::bytecode::{BytecodeBuilder, Opcode};
    use std::sync::{Arc, Mutex};

    fn log_interface() -> AbilityInterface {
        test_interface("Log", 4, &["debug", "info", "warn", "error"])
    }

    fn collecting_config(min_level: u8, output: &Arc<Mutex<Vec<(String, String)>>>) -> LogConfig {
        let output = Arc::clone(output);
        LogConfig {
            min_level,
            handler: Some(Box::new(move |level: &str, msg: &str| {
                output
                    .lock()
                    .expect("lock")
                    .push((level.to_string(), msg.to_string()));
            })),
        }
    }

    #[test]
    fn test_log_info() {
        let ability = log_interface();
        let output: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let config = collecting_config(0, &output);

        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::string("test message"));
        builder.emit_suspend(ability.id, require(&ability, "info"), 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_log(&mut vm, &ability, config);

        let result = vm.call(&hash, vec![]);
        assert_eq!(result, Ok(Value::Unit));

        let logs = output.lock().expect("lock");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0], ("INFO".to_string(), "test message".to_string()));
    }

    #[test]
    fn test_log_min_level_filters() {
        let ability = log_interface();
        let output: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));

        // Set min level to WARN (2), so DEBUG and INFO should be filtered
        let config = collecting_config(2, &output);

        let mut builder = BytecodeBuilder::new();

        // Log debug (should be filtered)
        builder.emit_const(Value::string("debug msg"));
        builder.emit_suspend(ability.id, require(&ability, "debug"), 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Pop);

        // Log warn (should pass)
        builder.emit_const(Value::string("warn msg"));
        builder.emit_suspend(ability.id, require(&ability, "warn"), 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Pop);

        // Log error (should pass)
        builder.emit_const(Value::string("error msg"));
        builder.emit_suspend(ability.id, require(&ability, "error"), 1);
        builder.emit(Opcode::Perform);

        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_log(&mut vm, &ability, config);

        let result = vm.call(&hash, vec![]);
        assert_eq!(result, Ok(Value::Unit));

        let logs = output.lock().expect("lock");
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0], ("WARN".to_string(), "warn msg".to_string()));
        assert_eq!(logs[1], ("ERROR".to_string(), "error msg".to_string()));
    }
}

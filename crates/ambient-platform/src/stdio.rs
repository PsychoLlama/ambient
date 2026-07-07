//! Stdio ability - stdin, stdout, and stderr for the host process.

#![allow(clippy::type_complexity)] // Handler types are inherently complex

use std::sync::{Arc, Mutex};

use ambient_ability::{SuspendedAbility, Value, VmError, format_value};
use ambient_engine::ability_resolver::AbilityInterface;
use ambient_engine::vm::Vm;

use crate::require;

/// The output half of stdio: a pair of line sinks for stdout and stderr.
///
/// `Log` writes through this same sink — the concrete realization of the
/// `ability Log with core::system::Stdio` dependency — so redirecting stdout
/// (e.g. to a collector) also captures log lines. Cloning shares the
/// underlying writers.
#[derive(Clone)]
pub struct StdioSink {
    out: Arc<dyn Fn(&str) + Send + Sync>,
    err: Arc<dyn Fn(&str) + Send + Sync>,
}

impl StdioSink {
    /// A sink over the process's real stdout and stderr.
    #[must_use]
    pub fn inherit() -> Self {
        Self {
            out: Arc::new(write_stdout),
            err: Arc::new(write_stderr),
        }
    }

    /// A sink with custom stdout / stderr line writers.
    #[must_use]
    pub fn new(out: Arc<dyn Fn(&str) + Send + Sync>, err: Arc<dyn Fn(&str) + Send + Sync>) -> Self {
        Self { out, err }
    }

    /// A sink that collects every stdout and stderr line into one buffer,
    /// stderr lines prefixed with `[ERR] `. For tests and embedders that
    /// capture output.
    #[must_use]
    pub fn collector(buffer: Arc<Mutex<Vec<String>>>) -> Self {
        let out_buf = Arc::clone(&buffer);
        Self {
            out: Arc::new(move |line: &str| {
                if let Ok(mut buf) = out_buf.lock() {
                    buf.push(line.to_string());
                }
            }),
            err: Arc::new(move |line: &str| {
                if let Ok(mut buf) = buffer.lock() {
                    buf.push(format!("[ERR] {line}"));
                }
            }),
        }
    }

    /// Write a line to the stdout stream.
    pub(crate) fn write_out(&self, line: &str) {
        (self.out)(line);
    }

    /// Write a line to the stderr stream.
    pub(crate) fn write_err(&self, line: &str) {
        (self.err)(line);
    }
}

impl Default for StdioSink {
    fn default() -> Self {
        Self::inherit()
    }
}

/// Configuration for the `Stdio` ability's input side.
#[derive(Default)]
pub struct StdioConfig {
    /// Custom reader for `read`. If `None`, reads a line from the process
    /// stdin. Returning `None` (or reaching end of input) surfaces to the
    /// language as the empty string.
    pub read_handler: Option<Box<dyn Fn() -> Option<String> + Send + Sync>>,
}

/// Register the `Stdio` ability handlers on a VM.
///
/// `out` and `err` write lines through `sink`; `read` pulls a line from
/// stdin (or `config.read_handler`).
///
/// # Panics
///
/// Panics if the resolved interface is missing an expected method — the
/// bindings interface and this handler set have drifted.
pub fn register_stdio(
    vm: &mut Vm,
    ability: &AbilityInterface,
    sink: StdioSink,
    config: StdioConfig,
) {
    // Stdio.out - write a line to stdout
    let out_sink = sink.clone();
    vm.register_host_handler(
        ability.id,
        require(ability, "out"),
        Box::new(move |ability: &SuspendedAbility| {
            let message = format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
            out_sink.write_out(&message);
            Ok(Value::Unit)
        }),
    );

    // Stdio.err - write a line to stderr
    vm.register_host_handler(
        ability.id,
        require(ability, "err"),
        Box::new(move |ability: &SuspendedAbility| {
            let message = format_value(&ability.args.first().cloned().unwrap_or(Value::Unit));
            sink.write_err(&message);
            Ok(Value::Unit)
        }),
    );

    // Stdio.read - read a line from stdin
    let read_handler = config.read_handler;
    vm.register_host_handler(
        ability.id,
        require(ability, "read"),
        Box::new(move |_ability: &SuspendedAbility| {
            if let Some(ref handler) = read_handler {
                return Ok(Value::string(handler().unwrap_or_default()));
            }
            read_stdin_line()
        }),
    );
}

/// Register `Stdio` with a collector buffer for its output (useful for
/// testing). `read` returns the empty string.
///
/// # Panics
///
/// Panics if the resolved interface is missing an expected method — the
/// bindings interface and this handler set have drifted.
pub fn register_stdio_with_collector(
    vm: &mut Vm,
    ability: &AbilityInterface,
    output: Arc<Mutex<Vec<String>>>,
) {
    register_stdio(
        vm,
        ability,
        StdioSink::collector(output),
        StdioConfig::default(),
    );
}

/// The default `read` behavior: one line from the process stdin, minus the
/// trailing newline. End of input is the empty string; a genuine IO error
/// is a catchable exception.
fn read_stdin_line() -> Result<Value, VmError> {
    #[cfg(not(test))]
    {
        use std::io::BufRead;
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) => Ok(Value::string("")),
            Ok(_) => Ok(Value::string(line.trim_end_matches(['\n', '\r']))),
            Err(e) => Err(VmError::exception(format!("Stdio.read: {e}"))),
        }
    }
    // Never block a test on real stdin.
    #[cfg(test)]
    {
        Ok(Value::string(""))
    }
}

/// Write a line to the real stdout (suppressed under `cfg(test)`).
fn write_stdout(line: &str) {
    #[cfg(not(test))]
    {
        #[allow(clippy::print_stdout)]
        {
            println!("{line}");
        }
    }
    #[cfg(test)]
    {
        let _ = line;
    }
}

/// Write a line to the real stderr (suppressed under `cfg(test)`).
fn write_stderr(line: &str) {
    #[cfg(not(test))]
    {
        #[allow(clippy::print_stderr)]
        {
            eprintln!("{line}");
        }
    }
    #[cfg(test)]
    {
        let _ = line;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_interface;
    use ambient_engine::bytecode::{BytecodeBuilder, Opcode};

    fn stdio_interface() -> AbilityInterface {
        test_interface("Stdio", 1, &["out", "err", "read"])
    }

    #[test]
    fn out_writes_through_the_sink() {
        let ability = stdio_interface();
        let output: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::string("Hello, World!"));
        builder.emit_suspend(ability.id, require(&ability, "out"), 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_stdio_with_collector(&mut vm, &ability, output.clone());

        let result = vm.call(&hash, vec![]);
        assert_eq!(result, Ok(Value::Unit));
        assert_eq!(*output.lock().expect("lock"), vec!["Hello, World!"]);
    }

    #[test]
    fn err_writes_through_the_sink_with_prefix() {
        let ability = stdio_interface();
        let output: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::string("boom"));
        builder.emit_suspend(ability.id, require(&ability, "err"), 1);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_stdio_with_collector(&mut vm, &ability, output.clone());

        let result = vm.call(&hash, vec![]);
        assert_eq!(result, Ok(Value::Unit));
        assert_eq!(*output.lock().expect("lock"), vec!["[ERR] boom"]);
    }

    #[test]
    fn read_uses_a_custom_handler() {
        let ability = stdio_interface();

        let mut builder = BytecodeBuilder::new();
        builder.emit_suspend(ability.id, require(&ability, "read"), 0);
        builder.emit(Opcode::Perform);
        builder.emit(Opcode::Return);

        let func = builder.build(0, 0);
        let hash = func.hash;

        let mut vm = Vm::new();
        vm.load_function(func);
        register_stdio(
            &mut vm,
            &ability,
            StdioSink::default(),
            StdioConfig {
                read_handler: Some(Box::new(|| Some("typed line".to_string()))),
            },
        );

        let result = vm.call(&hash, vec![]);
        assert_eq!(result, Ok(Value::string("typed line")));
    }
}

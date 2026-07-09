//! Stdio natives - stdin, stdout, and stderr for the host process.

use std::sync::{Arc, Mutex};

use ambient_ability::{Value, VmError, format_value};
use ambient_engine::natives::NativeRegistry;

use crate::bind;

/// The output half of stdio: a pair of line sinks for stdout and stderr.
///
/// `Log`'s default implementations perform `Stdio`, so its lines land in
/// this same sink — redirecting stdout (e.g. to a collector) also captures
/// log lines. Cloning shares the underlying writers.
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

/// Configuration for the `Stdio` natives' input side.
#[derive(Default)]
pub struct StdioConfig {
    /// Custom reader for `read`. If `None`, reads a line from the process
    /// stdin. Returning `None` (or reaching end of input) surfaces to the
    /// language as the empty string.
    pub read_handler: Option<Box<dyn Fn() -> Option<String> + Send + Sync>>,
}

/// The `Stdio` native implementations: `stdio_out` and `stdio_err` write
/// lines through `sink`; `stdio_read` pulls a line from stdin (or
/// `config.read_handler`).
#[must_use]
pub fn stdio_natives(sink: StdioSink, config: StdioConfig) -> NativeRegistry {
    let mut registry = NativeRegistry::new();

    let out_sink = sink.clone();
    bind(
        &mut registry,
        "stdio_out",
        Arc::new(move |args: Vec<Value>| {
            let message = format_value(args.first().unwrap_or(&Value::Unit));
            out_sink.write_out(&message);
            Ok(Value::Unit)
        }),
    );

    bind(
        &mut registry,
        "stdio_err",
        Arc::new(move |args: Vec<Value>| {
            let message = format_value(args.first().unwrap_or(&Value::Unit));
            sink.write_err(&message);
            Ok(Value::Unit)
        }),
    );

    let read_handler = config.read_handler;
    bind(
        &mut registry,
        "stdio_read",
        Arc::new(move |_args: Vec<Value>| {
            if let Some(ref handler) = read_handler {
                return Ok(Value::string(handler().unwrap_or_default()));
            }
            read_stdin_line()
        }),
    );

    registry
}

/// `Stdio` natives with a collector buffer for output (useful for
/// testing). `read` returns the empty string.
#[must_use]
pub fn stdio_natives_with_collector(output: Arc<Mutex<Vec<String>>>) -> NativeRegistry {
    stdio_natives(StdioSink::collector(output), StdioConfig::default())
}

/// The default `read` behavior: one line from the process stdin, minus the
/// trailing newline. End of input is the empty string; a genuine IO error
/// is a catchable exception.
// Under `cfg(test)` the body is infallible, but production reads real stdin
// and can fail, so the `Result` is load-bearing.
#[cfg_attr(test, allow(clippy::unnecessary_wraps))]
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
    use crate::native_uuid;

    /// Call a bound native by extern name.
    fn call(registry: &NativeRegistry, name: &str, args: Vec<Value>) -> Result<Value, VmError> {
        let func = registry
            .impl_for(&native_uuid(name))
            .unwrap_or_else(|| panic!("`{name}` is bound"));
        func(args)
    }

    #[test]
    fn out_writes_through_the_sink() {
        let output: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let registry = stdio_natives_with_collector(Arc::clone(&output));

        let result = call(&registry, "stdio_out", vec![Value::string("Hello, World!")]);
        assert_eq!(result, Ok(Value::Unit));
        assert_eq!(*output.lock().expect("lock"), vec!["Hello, World!"]);
    }

    #[test]
    fn err_writes_through_the_sink_with_prefix() {
        let output: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let registry = stdio_natives_with_collector(Arc::clone(&output));

        let result = call(&registry, "stdio_err", vec![Value::string("boom")]);
        assert_eq!(result, Ok(Value::Unit));
        assert_eq!(*output.lock().expect("lock"), vec!["[ERR] boom"]);
    }

    #[test]
    fn read_uses_a_custom_handler() {
        let registry = stdio_natives(
            StdioSink::default(),
            StdioConfig {
                read_handler: Some(Box::new(|| Some("typed line".to_string()))),
            },
        );

        let result = call(&registry, "stdio_read", vec![]);
        assert_eq!(result, Ok(Value::string("typed line")));
    }
}

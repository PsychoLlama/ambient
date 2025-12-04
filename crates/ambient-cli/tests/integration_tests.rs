//! Integration tests for the Ambient CLI.
//!
//! These tests verify the full compilation and execution pipeline.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use tempfile::TempDir;

/// Helper to run the ambient CLI command.
fn ambient_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ambient"))
}

/// Create a temporary directory with a source file.
fn temp_source(content: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("failed to create temp dir");
    let path = dir.path().join("test.ab");
    fs::write(&path, content).expect("failed to write source file");
    (dir, path)
}

// ─────────────────────────────────────────────────────────────────────────────
// Builder-style Test Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Builder for CLI integration tests.
///
/// Reduces boilerplate for common test patterns: creating temp files,
/// running CLI commands, and asserting on output.
struct CliTest {
    source: String,
    command: String,
    args: Vec<String>,
    _dir: Option<TempDir>,
    path: Option<PathBuf>,
}

#[allow(dead_code)]
impl CliTest {
    /// Create a new CLI test with the given source code.
    fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            command: "run".into(),
            args: Vec::new(),
            _dir: None,
            path: None,
        }
    }

    /// Use the "compile" command instead of "run".
    fn compile(mut self) -> Self {
        self.command = "compile".into();
        self
    }

    /// Use the "check" command instead of "run".
    fn check(mut self) -> Self {
        self.command = "check".into();
        self
    }

    /// Use the "ast" command instead of "run".
    fn ast(mut self) -> Self {
        self.command = "ast".into();
        self
    }

    /// Add additional arguments to the command.
    fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Execute the command and return the output.
    fn execute(&mut self) -> Output {
        let dir = TempDir::new().expect("failed to create temp dir");
        let path = dir.path().join("test.ab");
        fs::write(&path, &self.source).expect("failed to write source file");

        let mut cmd = ambient_cmd();
        cmd.arg(&self.command).arg(&path);
        for arg in &self.args {
            cmd.arg(arg);
        }

        self._dir = Some(dir);
        self.path = Some(path);

        cmd.output().expect("failed to execute command")
    }

    /// Execute and assert success with expected output in stdout.
    fn expect_output(mut self, expected: &str) {
        let output = self.execute();
        assert!(
            output.status.success(),
            "{} command failed: {:?}",
            self.command,
            output
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(expected),
            "expected '{}' in output: {}",
            expected,
            stdout
        );
    }

    /// Execute and assert success (no specific output check).
    fn expect_success(mut self) {
        let output = self.execute();
        assert!(
            output.status.success(),
            "{} command failed: {:?}",
            self.command,
            output
        );
    }

    /// Execute and assert failure.
    fn expect_failure(mut self) {
        let output = self.execute();
        assert!(
            !output.status.success(),
            "{} command should have failed",
            self.command
        );
    }

    /// Execute and assert failure with expected text in stderr.
    fn expect_error(mut self, expected: &str) {
        let output = self.execute();
        assert!(
            !output.status.success(),
            "{} command should have failed",
            self.command
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(expected),
            "expected '{}' in stderr: {}",
            expected,
            stderr
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Run Command Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_run_simple_return() {
    CliTest::new("fn main(): number { 42 }").expect_output("42");
}

#[test]
fn test_run_arithmetic() {
    CliTest::new("fn main(): number { 2 + 3 * 4 }").expect_output("14");
}

#[test]
fn test_run_boolean_logic() {
    CliTest::new("fn main(): bool { true && false || true }").expect_output("true");
}

#[test]
fn test_run_if_else() {
    CliTest::new(
        r#"
        fn main(): number {
            if 5 > 3 { 100 } else { 0 }
        }
    "#,
    )
    .expect_output("100");
}

#[test]
fn test_run_function_call() {
    CliTest::new(
        r#"
        fn double(x: number): number { x * 2 }
        fn main(): number { double(21) }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_run_recursive_factorial() {
    CliTest::new(
        r#"
        fn factorial(n: number): number {
            if n <= 1 { 1 } else { n * factorial(n - 1) }
        }
        fn main(): number { factorial(5) }
    "#,
    )
    .expect_output("120");
}

#[test]
fn test_run_multiple_functions() {
    CliTest::new(
        r#"
        fn add(a: number, b: number): number { a + b }
        fn square(x: number): number { x * x }
        fn main(): number { square(add(2, 3)) }
    "#,
    )
    .expect_output("25");
}

#[test]
fn test_run_let_binding() {
    CliTest::new(
        r#"
        fn main(): number {
            let x = 10;
            let y = 20;
            x + y
        }
    "#,
    )
    .expect_output("30");
}

#[test]
fn test_run_string_literal() {
    CliTest::new(r#"fn main(): string { "hello" }"#).expect_output("hello");
}

// ─────────────────────────────────────────────────────────────────────────────
// Compile Command Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compile_creates_output_file() {
    let (dir, path) = temp_source("fn main(): number { 42 }");
    let output_path = dir.path().join("test.ambient");

    let output = ambient_cmd()
        .arg("compile")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "command failed: {:?}", output);
    assert!(output_path.exists(), "output file not created");

    // Verify the output file contains valid JSON
    let contents = fs::read_to_string(&output_path).expect("failed to read output");
    let _: serde_json::Value = serde_json::from_str(&contents).expect("output is not valid JSON");

    drop(dir);
}

#[test]
fn test_compile_custom_output_path() {
    let (dir, path) = temp_source("fn main(): number { 42 }");
    let output_path = dir.path().join("custom.abc");

    let output = ambient_cmd()
        .arg("compile")
        .arg(&path)
        .arg("-o")
        .arg(&output_path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "command failed: {:?}", output);
    assert!(output_path.exists(), "custom output file not created");

    drop(dir);
}

#[test]
fn test_compile_then_run() {
    let (dir, path) = temp_source(
        r#"
        fn factorial(n: number): number {
            if n <= 1 { 1 } else { n * factorial(n - 1) }
        }
        fn main(): number { factorial(6) }
    "#,
    );
    let compiled_path = dir.path().join("test.ambient");

    // First compile
    let compile_output = ambient_cmd()
        .arg("compile")
        .arg(&path)
        .output()
        .expect("failed to execute compile command");

    assert!(
        compile_output.status.success(),
        "compile failed: {:?}",
        compile_output
    );
    assert!(compiled_path.exists(), "compiled file not created");

    // Then run the compiled file
    let run_output = ambient_cmd()
        .arg("run")
        .arg(&compiled_path)
        .output()
        .expect("failed to execute run command");

    assert!(run_output.status.success(), "run failed: {:?}", run_output);
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(
        stdout.contains("720"),
        "expected 720 (6!) in output: {stdout}"
    );

    drop(dir);
}

// ─────────────────────────────────────────────────────────────────────────────
// Check Command Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_check_valid_file() {
    CliTest::new(
        r#"
        fn add(a: number, b: number): number { a + b }
        fn main(): number { add(1, 2) }
    "#,
    )
    .check()
    .expect_success();
}

#[test]
fn test_check_invalid_syntax() {
    // Missing closing paren
    CliTest::new("fn main( { }").check().expect_failure();
}

// ─────────────────────────────────────────────────────────────────────────────
// AST Command Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_ast_output() {
    CliTest::new("fn main(): number { 42 }")
        .ast()
        .expect_output("main");
}

// ─────────────────────────────────────────────────────────────────────────────
// Error Handling Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_run_missing_main() {
    CliTest::new("fn other(): number { 42 }").expect_failure();
}

#[test]
fn test_run_nonexistent_file() {
    let output = ambient_cmd()
        .arg("run")
        .arg("/nonexistent/path/file.ab")
        .output()
        .expect("failed to execute command");

    assert!(!output.status.success(), "should fail for nonexistent file");
}

#[test]
fn test_run_wrong_extension() {
    let dir = TempDir::new().expect("failed to create temp dir");
    let path = dir.path().join("test.txt");
    fs::write(&path, "fn main(): number { 42 }").expect("failed to write");

    let output = ambient_cmd()
        .arg("run")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(!output.status.success(), "should fail for wrong extension");

    drop(dir);
}

// ─────────────────────────────────────────────────────────────────────────────
// Integration: Example Files
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_example_hello() {
    let output = ambient_cmd()
        .arg("run")
        .arg("../../examples/hello.ab")
        .output()
        .expect("failed to execute command");

    assert!(
        output.status.success(),
        "hello.ab should run successfully: {:?}",
        output
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("42"), "expected 42 in output: {stdout}");
}

#[test]
fn test_example_factorial() {
    let output = ambient_cmd()
        .arg("run")
        .arg("../../examples/factorial.ab")
        .output()
        .expect("failed to execute command");

    assert!(
        output.status.success(),
        "factorial.ab should run successfully: {:?}",
        output
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("120"), "expected 120 in output: {stdout}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler Value Tests (Milestone 13)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_handler_value_basic() {
    CliTest::new(
        r#"
        fn simple_function(): number { 100 }

        fn test_handler_value(): number {
            let mock_console = {
                print(msg) => resume(())
            };
            handle simple_function() with mock_console {}
        }

        fn main(): number { test_handler_value() }
    "#,
    )
    .expect_output("100");
}

#[test]
fn test_handler_value_multiple() {
    CliTest::new(
        r#"
        fn simple_function(): number { 200 }

        fn test_multiple_handlers(): number {
            let handler1 = { print(msg) => resume(()) };
            let handler2 = { throw(err) => resume(()) };
            handle simple_function() with handler1, handler2 {}
        }

        fn main(): number { test_multiple_handlers() }
    "#,
    )
    .expect_output("200");
}

#[test]
fn test_handler_value_with_inline() {
    CliTest::new(
        r#"
        fn simple_function(): number { 300 }

        fn test_mixed(): number {
            let mock_console = { print(msg) => resume(()) };
            handle simple_function() with mock_console {
                Exception.throw(err) => {
                    resume(())
                }
            }
        }

        fn main(): number { test_mixed() }
    "#,
    )
    .expect_output("300");
}

#[test]
fn test_example_handler_value() {
    let output = ambient_cmd()
        .arg("run")
        .arg("../../examples/handler_value_test.ab")
        .output()
        .expect("failed to execute command");

    assert!(
        output.status.success(),
        "handler_value_test.ab should run successfully: {:?}",
        output
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("100"), "expected 100 in output: {stdout}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Sandbox Tests (Milestone 14)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_sandbox_pure_computation() {
    CliTest::new(
        r#"
        fn pure_add(x: number, y: number): number {
            x + y
        }

        fn main(): number {
            sandbox {
                pure_add(2, 3)
            }
        }
    "#,
    )
    .expect_output("5");
}

#[test]
fn test_sandbox_with_allowed_ability() {
    CliTest::new(
        r#"
        fn compute(): number {
            42
        }

        fn main(): number {
            sandbox with Console {
                compute()
            }
        }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_sandbox_nested_pure() {
    CliTest::new(
        r#"
        fn factorial(n: number): number {
            if n <= 1 { 1 } else { n * factorial(n - 1) }
        }

        fn main(): number {
            sandbox {
                factorial(5)
            }
        }
    "#,
    )
    .expect_output("120");
}

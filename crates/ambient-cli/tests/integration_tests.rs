//! Integration tests for the Ambient CLI.
//!
//! These tests verify the full compilation and execution pipeline.

use std::process::Command;
use std::fs;
use tempfile::TempDir;

/// Helper to run the ambient CLI command.
fn ambient_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ambient"))
}

/// Create a temporary directory with a source file.
fn temp_source(content: &str) -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().expect("failed to create temp dir");
    let path = dir.path().join("test.ab");
    fs::write(&path, content).expect("failed to write source file");
    (dir, path)
}

// ─────────────────────────────────────────────────────────────────────────────
// Run Command Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_run_simple_return() {
    let (dir, path) = temp_source("fn main(): number { 42 }");

    let output = ambient_cmd()
        .arg("run")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "command failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("42"), "expected 42 in output: {stdout}");

    drop(dir);
}

#[test]
fn test_run_arithmetic() {
    let (dir, path) = temp_source("fn main(): number { 2 + 3 * 4 }");

    let output = ambient_cmd()
        .arg("run")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "command failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("14"), "expected 14 in output: {stdout}");

    drop(dir);
}

#[test]
fn test_run_boolean_logic() {
    let (dir, path) = temp_source("fn main(): bool { true && false || true }");

    let output = ambient_cmd()
        .arg("run")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "command failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("true"), "expected true in output: {stdout}");

    drop(dir);
}

#[test]
fn test_run_if_else() {
    let (dir, path) = temp_source(r#"
        fn main(): number {
            if 5 > 3 { 100 } else { 0 }
        }
    "#);

    let output = ambient_cmd()
        .arg("run")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "command failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("100"), "expected 100 in output: {stdout}");

    drop(dir);
}

#[test]
fn test_run_function_call() {
    let (dir, path) = temp_source(r#"
        fn double(x: number): number { x * 2 }
        fn main(): number { double(21) }
    "#);

    let output = ambient_cmd()
        .arg("run")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "command failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("42"), "expected 42 in output: {stdout}");

    drop(dir);
}

#[test]
fn test_run_recursive_factorial() {
    let (dir, path) = temp_source(r#"
        fn factorial(n: number): number {
            if n <= 1 { 1 } else { n * factorial(n - 1) }
        }
        fn main(): number { factorial(5) }
    "#);

    let output = ambient_cmd()
        .arg("run")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "command failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("120"), "expected 120 in output: {stdout}");

    drop(dir);
}

#[test]
fn test_run_multiple_functions() {
    let (dir, path) = temp_source(r#"
        fn add(a: number, b: number): number { a + b }
        fn square(x: number): number { x * x }
        fn main(): number { square(add(2, 3)) }
    "#);

    let output = ambient_cmd()
        .arg("run")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "command failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("25"), "expected 25 in output: {stdout}");

    drop(dir);
}

#[test]
fn test_run_let_binding() {
    let (dir, path) = temp_source(r#"
        fn main(): number {
            let x = 10;
            let y = 20;
            x + y
        }
    "#);

    let output = ambient_cmd()
        .arg("run")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "command failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("30"), "expected 30 in output: {stdout}");

    drop(dir);
}

#[test]
fn test_run_string_literal() {
    let (dir, path) = temp_source(r#"fn main(): string { "hello" }"#);

    let output = ambient_cmd()
        .arg("run")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "command failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("hello"), "expected hello in output: {stdout}");

    drop(dir);
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
    let (dir, path) = temp_source(r#"
        fn factorial(n: number): number {
            if n <= 1 { 1 } else { n * factorial(n - 1) }
        }
        fn main(): number { factorial(6) }
    "#);
    let compiled_path = dir.path().join("test.ambient");

    // First compile
    let compile_output = ambient_cmd()
        .arg("compile")
        .arg(&path)
        .output()
        .expect("failed to execute compile command");

    assert!(compile_output.status.success(), "compile failed: {:?}", compile_output);
    assert!(compiled_path.exists(), "compiled file not created");

    // Then run the compiled file
    let run_output = ambient_cmd()
        .arg("run")
        .arg(&compiled_path)
        .output()
        .expect("failed to execute run command");

    assert!(run_output.status.success(), "run failed: {:?}", run_output);
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(stdout.contains("720"), "expected 720 (6!) in output: {stdout}");

    drop(dir);
}

// ─────────────────────────────────────────────────────────────────────────────
// Check Command Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_check_valid_file() {
    let (dir, path) = temp_source(r#"
        fn add(a: number, b: number): number { a + b }
        fn main(): number { add(1, 2) }
    "#);

    let output = ambient_cmd()
        .arg("check")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "check should succeed for valid file: {:?}", output);

    drop(dir);
}

#[test]
fn test_check_invalid_syntax() {
    let (dir, path) = temp_source("fn main( { }"); // Missing closing paren

    let output = ambient_cmd()
        .arg("check")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(!output.status.success(), "check should fail for invalid syntax");

    drop(dir);
}

// ─────────────────────────────────────────────────────────────────────────────
// AST Command Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_ast_output() {
    let (dir, path) = temp_source("fn main(): number { 42 }");

    let output = ambient_cmd()
        .arg("ast")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "ast command failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Check that AST output contains expected elements
    assert!(stdout.contains("main"), "AST should contain function name");
    assert!(stdout.contains("FunctionDef") || stdout.contains("Function"),
            "AST should contain function definition");

    drop(dir);
}

// ─────────────────────────────────────────────────────────────────────────────
// Error Handling Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_run_missing_main() {
    let (dir, path) = temp_source("fn other(): number { 42 }");

    let output = ambient_cmd()
        .arg("run")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(!output.status.success(), "should fail when main is missing");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("main") || stderr.contains("entry"),
            "error should mention missing entry point: {stderr}");

    drop(dir);
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

    assert!(output.status.success(), "hello.ab should run successfully: {:?}", output);
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

    assert!(output.status.success(), "factorial.ab should run successfully: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("120"), "expected 120 in output: {stdout}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler Value Tests (Milestone 13)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_handler_value_basic() {
    // Test that handler values can be created and installed with `handle ... with`
    let (dir, path) = temp_source(r#"
        fn simple_function(): number { 100 }

        fn test_handler_value(): number {
            let mock_console = {
                print(msg) => resume(())
            };
            handle simple_function() with mock_console {}
        }

        fn main(): number { test_handler_value() }
    "#);

    let output = ambient_cmd()
        .arg("run")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "handler_value test failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("100"), "expected 100 in output: {stdout}");

    drop(dir);
}

#[test]
fn test_handler_value_multiple() {
    // Test multiple handler values
    let (dir, path) = temp_source(r#"
        fn simple_function(): number { 200 }

        fn test_multiple_handlers(): number {
            let handler1 = { print(msg) => resume(()) };
            let handler2 = { throw(err) => resume(()) };
            handle simple_function() with handler1, handler2 {}
        }

        fn main(): number { test_multiple_handlers() }
    "#);

    let output = ambient_cmd()
        .arg("run")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "multiple handler values test failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("200"), "expected 200 in output: {stdout}");

    drop(dir);
}

#[test]
fn test_handler_value_with_inline() {
    // Test combining handler value with inline handlers
    let (dir, path) = temp_source(r#"
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
    "#);

    let output = ambient_cmd()
        .arg("run")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "mixed handlers test failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("300"), "expected 300 in output: {stdout}");

    drop(dir);
}

#[test]
fn test_example_handler_value() {
    let output = ambient_cmd()
        .arg("run")
        .arg("../../examples/handler_value_test.ab")
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "handler_value_test.ab should run successfully: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("100"), "expected 100 in output: {stdout}");
}

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

/// Create a temporary package with the given source as main.ab.
fn temp_package(content: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("failed to create temp dir");
    let pkg_dir = dir.path().to_path_buf();

    // Create ambient.toml
    fs::write(
        pkg_dir.join("ambient.toml"),
        r#"[package]
name = "test_pkg"
version = "0.1.0"

[build]
src = "src"
"#,
    )
    .expect("failed to write manifest");

    // Create src/main.ab
    let src_dir = pkg_dir.join("src");
    fs::create_dir_all(&src_dir).expect("failed to create src dir");
    fs::write(src_dir.join("main.ab"), content).expect("failed to write source file");

    (dir, pkg_dir)
}

/// Create a temporary directory with a single source file (for check/compile/ast).
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
/// Reduces boilerplate for common test patterns: creating temp packages,
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
        let mut cmd = ambient_cmd();

        // For run command, create a full package
        // For other commands, create just a source file
        if self.command == "run" {
            let (dir, pkg_path) = temp_package(&self.source);
            cmd.arg(&self.command).arg(&pkg_path);
            self._dir = Some(dir);
            self.path = Some(pkg_path);
        } else {
            let (dir, file_path) = temp_source(&self.source);
            cmd.arg(&self.command).arg(&file_path);
            self._dir = Some(dir);
            self.path = Some(file_path);
        }

        for arg in &self.args {
            cmd.arg(arg);
        }

        cmd.output().expect("failed to execute command")
    }

    /// Execute and assert success with expected output in stdout.
    fn expect_output(mut self, expected: &str) {
        let output = self.execute();
        assert!(
            output.status.success(),
            "{} command failed: {:?}\nstderr: {}",
            self.command,
            output,
            String::from_utf8_lossy(&output.stderr)
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
            "{} command failed: {:?}\nstderr: {}",
            self.command,
            output,
            String::from_utf8_lossy(&output.stderr)
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
    CliTest::new("fn run(): Number { 42 }").expect_output("42");
}

/// Single-file `run` (no synthesized package) must link core-library inherent
/// methods. `.length()` lowers to the dispatch symbol `List::length`, which is
/// globally unique and already qualified, so the link table must bind it
/// unprefixed. Regression for the drifted duplicate of `build::linking_table`
/// that double-qualified it (`core::collections::List::List::length`). The `CliTest` harness
/// package-wraps every `run`, so this bypasses it to exercise the bare-file path.
#[test]
fn single_file_run_links_core_inherent_methods() {
    let (_dir, path) = temp_source("fn run(): Number { [1, 2, 3].length() }");
    let out = ambient_cmd().arg("run").arg(&path).output().expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains('3'), "expected '3' in output: {stdout}");
}

#[test]
fn test_run_arithmetic() {
    CliTest::new("fn run(): Number { 2 + 3 * 4 }").expect_output("14");
}

// ─────────────────────────────────────────────────────────────────────────────
// Block-scoped `const` Tests
// ─────────────────────────────────────────────────────────────────────────────

/// A `const` declared inside a function body binds a name for the rest of the
/// block and is usable like any value.
#[test]
fn block_const_is_referenced_within_a_function() {
    CliTest::new("fn run(): Number { const X = 40; X + 2 }").expect_output("42");
}

/// The block `const`'s type annotation is optional (inferred from the literal)
/// but still accepted when written.
#[test]
fn block_const_accepts_explicit_type() {
    CliTest::new("fn run(): Number { const X: Number = 42; X }").expect_output("42");
}

/// Block consts carry non-numeric literals too.
#[test]
fn block_const_binds_a_string() {
    CliTest::new(r#"fn run(): String { const GREETING = "hello"; GREETING }"#)
        .expect_output("hello");
}

/// An enclosing block `const` is visible inside a nested lambda — the
/// reference loads the value object by hash, needing no capture slot.
#[test]
fn block_const_visible_inside_nested_lambda() {
    CliTest::new("fn run(): Number { const BONUS = 10; let add = (n) => n + BONUS; add(32) }")
        .expect_output("42");
}

/// A block `const` shadows a module-level binding of the same name from its
/// declaration onward.
#[test]
fn block_const_shadows_module_const() {
    CliTest::new("const N: Number = 1;\nfn run(): Number { const N = 42; N }").expect_output("42");
}

/// Referencing a block `const` before its declaration is an error, exactly
/// like a `let` (no forward-reference pass).
#[test]
fn block_const_referenced_before_declaration_is_error() {
    CliTest::new("fn run(): Number { let y = MISSING; const MISSING = 5; y }")
        .check()
        .expect_failure();
}

#[test]
fn test_run_boolean_logic() {
    CliTest::new("fn run(): Bool { true && false || true }").expect_output("true");
}

#[test]
fn test_run_if_else() {
    CliTest::new(
        r#"
        fn run(): Number {
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
        fn double(x: Number): Number { x * 2 }
        fn run(): Number { double(21) }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_run_recursive_factorial() {
    CliTest::new(
        r#"
        fn factorial(n: Number): Number {
            if n <= 1 { 1 } else { n * factorial(n - 1) }
        }
        fn run(): Number { factorial(5) }
    "#,
    )
    .expect_output("120");
}

#[test]
fn test_run_multiple_functions() {
    CliTest::new(
        r#"
        fn add(a: Number, b: Number): Number { a + b }
        fn square(x: Number): Number { x * x }
        fn run(): Number { square(add(2, 3)) }
    "#,
    )
    .expect_output("25");
}

#[test]
fn test_run_let_binding() {
    CliTest::new(
        r#"
        fn run(): Number {
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
    CliTest::new(r#"fn run(): String { "hello" }"#).expect_output("hello");
}

// ─────────────────────────────────────────────────────────────────────────────
// Compile Command Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_compile_creates_output_file() {
    let (dir, path) = temp_source("fn run(): Number { 42 }");
    let output_path = dir.path().join("test.ambient");

    let output = ambient_cmd()
        .arg("compile")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(output.status.success(), "command failed: {:?}", output);
    assert!(output_path.exists(), "output file not created");

    // The artifact is a binary pack; running it must produce the result.
    let run_output = ambient_cmd()
        .arg("run")
        .arg(&output_path)
        .output()
        .expect("failed to execute run");
    assert!(
        run_output.status.success(),
        "running the artifact failed: {run_output:?}"
    );
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(
        stdout.contains("42"),
        "artifact run produced unexpected output: {stdout}"
    );

    drop(dir);
}

#[test]
fn test_compile_custom_output_path() {
    let (dir, path) = temp_source("fn run(): Number { 42 }");
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
        fn factorial(n: Number): Number {
            if n <= 1 { 1 } else { n * factorial(n - 1) }
        }
        fn run(): Number { factorial(6) }
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
        fn add(a: Number, b: Number): Number { a + b }
        fn run(): Number { add(1, 2) }
    "#,
    )
    .check()
    .expect_success();
}

#[test]
fn test_check_invalid_syntax() {
    // Missing closing paren
    CliTest::new("fn run( { }").check().expect_failure();
}

// ─────────────────────────────────────────────────────────────────────────────
// AST Command Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_ast_output() {
    CliTest::new("fn run(): Number { 42 }")
        .ast()
        .expect_output("run");
}

// ─────────────────────────────────────────────────────────────────────────────
// Error Handling Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_run_missing_run_function() {
    // Package with no run function
    CliTest::new("fn other(): Number { 42 }").expect_failure();
}

#[test]
fn test_run_nonexistent_package() {
    let output = ambient_cmd()
        .arg("run")
        .arg("/nonexistent/path/")
        .output()
        .expect("failed to execute command");

    assert!(
        !output.status.success(),
        "should fail for nonexistent package"
    );
}

#[test]
fn test_run_non_package_file() {
    // Trying to run a regular file that's not a .ambient bytecode file
    let dir = TempDir::new().expect("failed to create temp dir");
    let path = dir.path().join("test.txt");
    fs::write(&path, "fn run(): Number { 42 }").expect("failed to write");

    let output = ambient_cmd()
        .arg("run")
        .arg(&path)
        .output()
        .expect("failed to execute command");

    assert!(!output.status.success(), "should fail for non-package file");

    drop(dir);
}

// ─────────────────────────────────────────────────────────────────────────────
// Integration: Example Packages
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_example_hello() {
    let output = ambient_cmd()
        .arg("run")
        .arg("../../examples/hello")
        .output()
        .expect("failed to execute command");

    assert!(
        output.status.success(),
        "hello package should run successfully: {:?}\nstderr: {}",
        output,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("42"), "expected 42 in output: {stdout}");
}

#[test]
fn test_example_factorial() {
    let output = ambient_cmd()
        .arg("run")
        .arg("../../examples/factorial")
        .output()
        .expect("failed to execute command");

    assert!(
        output.status.success(),
        "factorial package should run successfully: {:?}\nstderr: {}",
        output,
        String::from_utf8_lossy(&output.stderr)
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
        fn simple_function(): Number { 100 }

        fn test_handler_value(): Number {
            let mock_console = {
                core::system::Stdio::out(msg) => resume(())
            };
            with mock_console handle simple_function()
        }

        fn run(): Number { test_handler_value() }
    "#,
    )
    .expect_output("100");
}

#[test]
fn test_handler_value_multiple() {
    CliTest::new(
        r#"
        fn simple_function(): Number { 200 }

        fn test_multiple_handlers(): Number {
            let handler1 = { core::system::Stdio::out(msg) => resume(()) };
            let handler2 = { Exception::throw(err) => resume(()) };
            with handler1, handler2 handle simple_function()
        }

        fn run(): Number { test_multiple_handlers() }
    "#,
    )
    .expect_output("200");
}

#[test]
fn test_handler_value_with_inline() {
    CliTest::new(
        r#"
        fn simple_function(): Number { 300 }

        fn test_mixed(): Number {
            let mock_console = { core::system::Stdio::out(msg) => resume(()) };
            with mock_console, {
                Exception::throw(err) => {
                    resume(())
                }
            } handle simple_function()
        }

        fn run(): Number { test_mixed() }
    "#,
    )
    .expect_output("300");
}

/// Composing a handler value with an inline override for the *same* ability
/// installs left-to-right, so the later handler wins ("last wins"). Here the
/// inline `resume(2)` shadows the value's `resume(1)`.
#[test]
fn test_handler_value_override_last_wins() {
    CliTest::new(
        r#"
        ability Choice {
          fn pick(): Number;
        }

        fn body(): Number with Choice {
          Choice::pick!()
        }

        fn test(): Number {
          let first = { Choice::pick() => resume(1) };
          with first, { Choice::pick() => resume(2) } handle body()
        }

        fn run(): Number { test() }
    "#,
    )
    .expect_output("2");
}

/// A handler *value* (let-bound, so it installs through the `HandlerValue`
/// path) may capture variables from its enclosing scope. The arm body reads
/// `offset`, a local of the surrounding function, proving the captured
/// environment reaches the method function at runtime.
#[test]
fn test_handler_value_captures_outer_variable() {
    CliTest::new(
        r#"
        ability Choice {
          fn pick(): Number;
        }

        fn body(): Number with Choice {
          Choice::pick!()
        }

        fn test(): Number {
          let offset = 40;
          let handler = { Choice::pick() => resume(offset + 2) };
          with handler handle body()
        }

        fn run(): Number { test() }
    "#,
    )
    .expect_output("42");
}

/// Two arms of one handler value capture *different* outer variables. They
/// share one runtime capture array, so each name must land on its own
/// stable slot; whichever arm fires reads the right value.
#[test]
fn test_handler_value_multi_arm_captures() {
    CliTest::new(
        r#"
        ability Pair {
          fn left(): Number;
          fn right(): Number;
        }

        fn body(): Number with Pair {
          Pair::left!() + Pair::right!()
        }

        fn test(): Number {
          let a = 10;
          let b = 20;
          let handler = {
            Pair::left() => resume(a),
            Pair::right() => resume(b)
          };
          with handler handle body()
        }

        fn run(): Number { test() }
    "#,
    )
    .expect_output("30");
}

/// An inline multi-method brace for one ability installs a single
/// method-dispatched `HandlerValue`, so each perform reaches its own arm.
/// `left` yields 1 and `right` yields 2, giving `1*10 + 2`; a
/// method-agnostic install (the old per-arm `Handle` path) would run one
/// arm for both performs.
#[test]
fn test_inline_multi_method_dispatch() {
    CliTest::new(
        r#"
        ability Pair {
          fn left(): Number;
          fn right(): Number;
        }

        fn body(): Number with Pair {
          Pair::left!() * 10 + Pair::right!()
        }

        fn run(): Number {
          with { Pair::left() => resume(1), Pair::right() => resume(2) } handle body()
        }
    "#,
    )
    .expect_output("12");
}

#[test]
fn test_example_handler_value() {
    let output = ambient_cmd()
        .arg("run")
        .arg("../../examples/handler_value_test")
        .output()
        .expect("failed to execute command");

    assert!(
        output.status.success(),
        "handler_value_test package should run successfully: {:?}\nstderr: {}",
        output,
        String::from_utf8_lossy(&output.stderr)
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
        fn pure_add(x: Number, y: Number): Number {
            x + y
        }

        fn run(): Number {
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
        fn compute(): Number {
            42
        }

        fn run(): Number {
            sandbox with core::system::Stdio {
                compute()
            }
        }
    "#,
    )
    .expect_output("42");
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform ability namespace policy: platform abilities must be written
// `core::system::X` in every position (with clauses, effect rows, handler
// arms, sandbox clauses, performs); user abilities and Exception stay
// bare.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_with_clause_requires_platform_namespace() {
    CliTest::new(
        r#"
        pub fn run(): () with Stdio {
            core::system::Stdio::out!("hi")
        }
        "#,
    )
    .expect_error("qualify it as `core::system::");
}

#[test]
fn test_handler_arm_requires_platform_namespace() {
    CliTest::new(
        r#"
        fn speak(): () with core::system::Stdio {
            core::system::Stdio::out!("hi")
        }

        pub fn run(): () {
            with {
                Stdio::out(msg) => {
                    resume(())
                }
            } handle speak()
        }
        "#,
    )
    .expect_error("qualify it as `core::system::");
}

#[test]
fn test_sandbox_clause_requires_platform_namespace() {
    CliTest::new(
        r#"
        pub fn run(): Number {
            sandbox with Stdio {
                42
            }
        }
        "#,
    )
    .expect_error("qualify it as `core::system::");
}

#[test]
fn test_effect_row_annotation_requires_platform_namespace() {
    CliTest::new(
        r#"
        fn call(f: () -> () with Log): () with core::system::Log {
            f()
        }

        pub fn run(): () {
            ()
        }
        "#,
    )
    .expect_error("qualify it as `core::system::");
}

#[test]
fn test_exception_may_not_be_namespaced() {
    // Exception is a language builtin, never platform-qualified.
    CliTest::new(
        r#"
        pub fn run(): () with core::system::Exception {
            ()
        }
        "#,
    )
    .expect_error("unknown ability");
}

#[test]
fn test_local_ability_shadows_platform_name() {
    // A local declaration reclaims the bare name in every position; the
    // platform ability stays reachable through its prefix.
    CliTest::new(
        r#"
        ability Stdio {
            fn shout(message: String): String;
        }

        fn noise(): String with Stdio {
            Stdio::shout!("quiet")
        }

        pub fn run(): () with core::system::Stdio {
            let loud = with {
                Stdio::shout(msg) => {
                    resume(core::primitives::String::concat(msg, "!"))
                }
            } handle noise();
            core::system::Stdio::out!(loud)
        }
        "#,
    )
    .expect_output("quiet!");
}

#[test]
fn test_time_wait_accepts_duration() {
    // `core::system::Time::wait` takes a `core::time::Duration`. This exercises
    // the whole path: the ability signature references a core nominal type,
    // the checker unifies the caller's `Duration` against the signature's
    // (unexpanded) reference, and the host handler decodes the record and
    // sleeps. A successful numeric result proves compile *and* dispatch.
    CliTest::new(
        r#"
        use core::time::Duration;

        pub fn run(): Number with core::system::Time {
            core::system::Time::wait!(Duration::from_millis(1));
            7
        }
        "#,
    )
    .expect_output("7");
}

#[test]
fn test_time_wait_rejects_raw_number() {
    // A bare millisecond count is no longer accepted: the Named/Nominal
    // bridge only unifies the signature's `Duration` against a real
    // `Duration`, not against `number`.
    CliTest::new(
        r#"
        pub fn run(): () with core::system::Time {
            core::system::Time::wait!(20);
        }
        "#,
    )
    .expect_error("type mismatch");
}

#[test]
fn test_sandbox_nested_pure() {
    CliTest::new(
        r#"
        fn factorial(n: Number): Number {
            if n <= 1 { 1 } else { n * factorial(n - 1) }
        }

        fn run(): Number {
            sandbox {
                factorial(5)
            }
        }
    "#,
    )
    .expect_output("120");
}

// ─────────────────────────────────────────────────────────────────────────────
// Error Message Tests (Ticket 5.3)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_error_parse_missing_brace() {
    // Missing closing brace should produce a parse error
    let (_dir, path) = temp_source(
        r#"
        fn run(): Number {
            42
        // missing closing brace
    "#,
    );

    let output = ambient_cmd()
        .arg("check")
        .arg(&path)
        .output()
        .expect("failed to run command");

    assert!(!output.status.success(), "should fail with parse error");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error") || stderr.contains("Error"),
        "stderr should mention error: {}",
        stderr
    );
}

#[test]
fn test_error_type_mismatch() {
    // Type mismatch should produce a type error
    let (_dir, path) = temp_source(
        r#"
        fn run(): Number {
            "hello"
        }
    "#,
    );

    let output = ambient_cmd()
        .arg("check")
        .arg(&path)
        .output()
        .expect("failed to run command");

    assert!(!output.status.success(), "should fail with type error");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("type") || stderr.contains("mismatch") || stderr.contains("error"),
        "stderr should mention type error: {}",
        stderr
    );
}

#[test]
fn test_error_undefined_function() {
    // Calling undefined function should produce an error
    let (_dir, path) = temp_source(
        r#"
        fn run(): Number {
            undefined_function()
        }
    "#,
    );

    let output = ambient_cmd()
        .arg("check")
        .arg(&path)
        .output()
        .expect("failed to run command");

    assert!(
        !output.status.success(),
        "should fail with undefined function error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("undefined") || stderr.contains("not found") || stderr.contains("error"),
        "stderr should mention undefined: {}",
        stderr
    );
}

#[test]
fn test_error_wrong_argument_count() {
    // Calling function with wrong number of args
    let (_dir, path) = temp_source(
        r#"
        fn add(x: Number, y: Number): Number {
            x + y
        }

        fn run(): Number {
            add(1)
        }
    "#,
    );

    let output = ambient_cmd()
        .arg("check")
        .arg(&path)
        .output()
        .expect("failed to run command");

    assert!(
        !output.status.success(),
        "should fail with argument count error"
    );
}

#[test]
fn test_end_to_end_tuples() {
    // Test tuple creation and access through full pipeline
    CliTest::new(
        r#"
        fn run(): Number {
            let t = (1, 2, 3);
            t.0 + t.1 + t.2
        }
    "#,
    )
    .expect_output("6");
}

#[test]
fn test_end_to_end_records() {
    // Test record creation through full pipeline
    // Note: record field access from variables (r.x) is not yet supported by parser
    // (parsed as qualified name - see ticket 3.1)
    CliTest::new(
        r#"
        fn run(): Number {
            let _r = { x: 10, y: 20 };
            30
        }
    "#,
    )
    .expect_output("30");
}

#[test]
fn test_end_to_end_lists() {
    // Test list creation through full pipeline
    CliTest::new(
        r#"
        fn run(): Number {
            let xs = [1, 2, 3];
            3
        }
    "#,
    )
    .expect_output("3");
}

#[test]
fn test_end_to_end_match() {
    // Test match expression through full pipeline
    CliTest::new(
        r#"
        fn classify(n: Number): Number {
            match n {
                0 => 0,
                1 => 1,
                _ => 2,
            }
        }

        fn run(): Number {
            classify(5)
        }
    "#,
    )
    .expect_output("2");
}

#[test]
fn test_end_to_end_closure() {
    // Test closure capture through full pipeline
    CliTest::new(
        r#"
        fn run(): Number {
            let multiplier = 10;
            let f = (x: Number) => x * multiplier;
            f(5)
        }
    "#,
    )
    .expect_output("50");
}

#[test]
fn test_end_to_end_nested_calls() {
    // Test nested function calls through full pipeline
    CliTest::new(
        r#"
        fn double(x: Number): Number { x * 2 }
        fn add_one(x: Number): Number { x + 1 }

        fn run(): Number {
            add_one(double(add_one(double(5))))
        }
    "#,
    )
    .expect_output("23");
}

#[test]
fn test_end_to_end_mutual_recursion() {
    // Test mutual recursion through full pipeline
    CliTest::new(
        r#"
        fn is_even(n: Number): Bool {
            if n == 0 { true } else { is_odd(n - 1) }
        }

        fn is_odd(n: Number): Bool {
            if n == 0 { false } else { is_even(n - 1) }
        }

        fn run(): Bool {
            is_even(10)
        }
    "#,
    )
    .expect_output("true");
}

#[test]
fn test_end_to_end_higher_order() {
    // Test higher-order functions through full pipeline
    CliTest::new(
        r#"
        fn apply_twice(f: (Number) -> Number, x: Number): Number {
            f(f(x))
        }

        fn run(): Number {
            let double = (x: Number) => x * 2;
            apply_twice(double, 3)
        }
    "#,
    )
    .expect_output("12");
}

#[test]
fn test_end_to_end_complex_expression() {
    // Test complex nested expression through full pipeline
    CliTest::new(
        r#"
        fn run(): Number {
            let x = (1 + 2) * 3;
            let y = if x > 5 { x * 2 } else { x };
            y + 1
        }
    "#,
    )
    .expect_output("19");
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait System Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_trait_definition_and_impl() {
    // Test trait definition and implementation for a nominal type
    CliTest::new(
        r#"
        trait Show {
            fn show(self): Number;
        }

        unique(A1B2C3D4-0000-0000-0000-000000000001) struct Counter { value: Number }

        impl Show for Counter {
            fn show(self): Number {
                self.value
            }
        }

        fn run(): Number {
            let c = Counter { value: 42 };
            c.show()
        }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_trait_method_with_args() {
    // Test trait method that takes additional arguments
    CliTest::new(
        r#"
        trait Scalable {
            fn scale(self, factor: Number): Number;
        }

        unique(A1B2C3D4-0000-0000-0000-000000000002) struct Size { width: Number }

        impl Scalable for Size {
            fn scale(self, factor: Number): Number {
                self.width * factor
            }
        }

        fn run(): Number {
            let s = Size { width: 10 };
            s.scale(5)
        }
    "#,
    )
    .expect_output("50");
}

#[test]
fn test_operator_overloading_add() {
    // Test Add trait for operator overloading
    CliTest::new(
        r#"
        trait Add {
            fn add(self, other: Self): Self;
        }

        unique(A1B2C3D4-0000-0000-0000-000000000003) struct Money { cents: Number }

        impl Add for Money {
            fn add(self, other: Money): Money {
                Money { cents: self.cents + other.cents }
            }
        }

        fn run(): Number {
            let a = Money { cents: 100 };
            let b = Money { cents: 50 };
            let total = a + b;
            total.cents
        }
    "#,
    )
    .expect_output("150");
}

#[test]
fn test_operator_overloading_eq() {
    // Test Eq trait for equality comparison
    CliTest::new(
        r#"
        trait Eq {
            fn eq(self, other: Self): Bool;
        }

        unique(A1B2C3D4-0000-0000-0000-000000000004) struct Id { value: Number }

        impl Eq for Id {
            fn eq(self, other: Id): Bool {
                self.value == other.value
            }
        }

        fn run(): Bool {
            let a = Id { value: 42 };
            let b = Id { value: 42 };
            a == b
        }
    "#,
    )
    .expect_output("true");
}

#[test]
fn test_default_trait_associated_call() {
    // The prelude `Default` trait provides an associated (no-`self`)
    // `default(): Self`, invoked as `Type::default()`.
    CliTest::new(
        r#"
        unique(A1B2C3D4-0000-0000-0000-000000000010) struct Config { level: Number }

        impl Default for Config {
            fn default(): Config {
                Config { level: 7 }
            }
        }

        fn run(): Number {
            let c = Config::default();
            c.level
        }
    "#,
    )
    .expect_output("7");
}

#[test]
fn test_default_trait_composes_with_operator() {
    // An associated call is an ordinary expression: it nests and composes
    // with operators like any other value.
    CliTest::new(
        r#"
        unique(A1B2C3D4-0000-0000-0000-000000000011) struct Vec2 { x: Number, y: Number }

        impl Default for Vec2 {
            fn default(): Vec2 {
                Vec2 { x: 0, y: 0 }
            }
        }

        impl Add for Vec2 {
            fn add(self, other: Vec2): Vec2 {
                Vec2 { x: self.x + other.x, y: self.y + other.y }
            }
        }

        fn run(): Number {
            let v = Vec2::default() + Vec2 { x: 3, y: 4 };
            v.x + v.y
        }
    "#,
    )
    .expect_output("7");
}

#[test]
fn test_associated_trait_method_with_argument() {
    // The associated-call mechanism is not special to `Default`: any
    // user-declared trait method without `self` is callable as
    // `Type::method(args)`.
    CliTest::new(
        r#"
        trait FromNumber {
            fn from_number(n: Number): Self;
        }

        unique(A1B2C3D4-0000-0000-0000-000000000012) struct Wrapped { value: Number }

        impl FromNumber for Wrapped {
            fn from_number(n: Number): Wrapped {
                Wrapped { value: n * 2 }
            }
        }

        fn run(): Number {
            let w = Wrapped::from_number(21);
            w.value
        }
    "#,
    )
    .expect_output("42");
}

// ─────────────────────────────────────────────────────────────────────────────
// Inherent Impl Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_inherent_impl_method_call() {
    // `impl Type { ... }` attaches methods directly to a nominal type;
    // dot dispatch resolves them without any trait.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-000000000001) struct Money { cents: Number }

        impl Money {
            fn double(self): Money {
                Money { cents: self.cents * 2 }
            }
        }

        fn run(): Number {
            let m = Money { cents: 21 };
            m.double().cents
        }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_inherent_impl_associated_call() {
    // A no-`self` inherent method is an associated function, called as
    // `Type::method(args)` — no trait declaration needed.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-000000000002) struct Money { cents: Number }

        impl Money {
            fn from_dollars(d: Number): Money {
                Money { cents: d * 100 }
            }
            fn cents(self): Number {
                self.cents
            }
        }

        fn run(): Number {
            Money::from_dollars(3).cents()
        }
    "#,
    )
    .expect_output("300");
}

#[test]
fn test_inherent_impl_methods_call_each_other() {
    // Inherent method signatures register before bodies are checked, so
    // methods can call each other regardless of declaration order.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-000000000003) struct Counter { n: Number }

        impl Counter {
            fn bump_twice(self): Counter {
                self.bump().bump()
            }
            fn bump(self): Counter {
                Counter { n: self.n + 1 }
            }
        }

        fn run(): Number {
            let c = Counter { n: 0 };
            c.bump_twice().n
        }
    "#,
    )
    .expect_output("2");
}

#[test]
fn test_inherent_impl_generic_option() {
    // Generic inherent impls attach methods to built-in type constructors.
    // The receiver's type arguments instantiate the impl's parameters.
    CliTest::new(
        r#"
        impl<T> Option<T> {
            fn get_or(self, fallback: T): T {
                match self {
                    Some(v) => v,
                    None => fallback,
                }
            }
        }

        fn run(): Number {
            let a: Option<Number> = Some(40);
            let b: Option<Number> = None;
            a.get_or(0) + b.get_or(2)
        }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_inherent_impl_generic_method_on_user_enum() {
    // A generic method (its own type parameter, beyond the impl's) on a
    // user-declared enum.
    CliTest::new(
        r#"
        unique(C1B2C3D4-0000-0000-0000-000000000010) enum Box2 { Full(Number), Empty }

        impl Box2 {
            fn map_or<U>(self, fallback: U, f: (Number) -> U): U {
                match self {
                    Full(v) => f(v),
                    Empty => fallback,
                }
            }
        }

        fn run(): Number {
            let b = Full(20);
            b.map_or(0, (v) => v * 2) + Empty.map_or(2, (v) => v)
        }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_unit_struct_constructed_by_bare_name() {
    // A unit struct (`unique(...) struct Origin;`) is constructed by its bare
    // name — `Origin` *is* the sole value of its type, like a nullary enum
    // variant. It flows through a parameter of its own nominal type, proving
    // parse → resolve → check → compile → run.
    CliTest::new(
        r#"
        unique(C1B2C3D4-0000-0000-0000-000000000030) struct Origin;

        fn accept(_o: Origin): Number { 7 }

        fn run(): Number {
            let o = Origin;
            accept(o)
        }
    "#,
    )
    .expect_output("7");
}

#[test]
fn test_field_bearing_struct_used_bare_is_undefined() {
    // Only *unit* structs gain a value binding. A field-bearing struct named
    // in value position is not a value — it still fails as an undefined
    // variable, guarding against over-eager binding.
    CliTest::new(
        r#"
        unique(C1B2C3D4-0000-0000-0000-000000000031) struct Point { x: Number }

        fn run(): Number {
            let p = Point;
            0
        }
    "#,
    )
    .check()
    .expect_error("undefined variable: `Point`");
}

#[test]
fn test_inherent_method_with_ability() {
    // Inherent methods declare effects like public functions: a `with`
    // clause on the method, enforced on the body and required at call
    // sites.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-000000000004) struct Greeter { name: String }

        impl Greeter {
            fn greet(self): () with core::system::Stdio {
                core::system::Stdio::out!("hello ${self.name}");
            }
        }

        pub fn run(): () with core::system::Stdio {
            let g = Greeter { name: "world" };
            g.greet();
        }
    "#,
    )
    .expect_output("hello world");
}

#[test]
fn test_inherent_method_undeclared_ability_error() {
    // A pure-signature inherent method whose body performs an ability is
    // rejected, exactly like a public function.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-000000000005) struct Greeter { name: String }

        impl Greeter {
            fn greet(self): () {
                core::system::Stdio::out!("hello");
            }
        }

        fn run(): () {
            let g = Greeter { name: "x" };
            g.greet();
        }
    "#,
    )
    .expect_error("uses ability");
}

#[test]
fn test_inherent_method_ability_required_at_call_site() {
    // The method's declared abilities propagate to callers: a pure public
    // function cannot call an effectful method.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-000000000006) struct Greeter { name: String }

        impl Greeter {
            fn greet(self): () with core::system::Stdio {
                core::system::Stdio::out!("hello");
            }
        }

        pub fn run(): () {
            let g = Greeter { name: "x" };
            g.greet();
        }
    "#,
    )
    .expect_error("uses ability");
}

#[test]
fn test_duplicate_inherent_method_error() {
    // Two definitions of the same method for the same type would compete
    // for one dispatch symbol; coherence rejects the second.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-000000000007) struct Money { cents: Number }

        impl Money {
            fn double(self): Money {
                Money { cents: self.cents * 2 }
            }
        }

        impl Money {
            fn double(self): Money {
                Money { cents: self.cents * 4 }
            }
        }

        fn run(): Number {
            Money { cents: 1 }.double().cents
        }
    "#,
    )
    .expect_error("duplicate inherent method");
}

#[test]
fn test_inherent_method_shadows_trait_method() {
    // Dispatch precedence: inherent methods win over same-named trait
    // methods (like Rust), so adding an inherent method is a deliberate
    // local override rather than an ambiguity error.
    CliTest::new(
        r#"
        trait Doubler {
            fn double(self): Self;
        }

        unique(B1B2C3D4-0000-0000-0000-000000000008) struct Num { val: Number }

        impl Doubler for Num {
            fn double(self): Num {
                Num { val: self.val * 2 }
            }
        }

        impl Num {
            fn double(self): Num {
                Num { val: self.val * 10 }
            }
        }

        fn run(): Number {
            let n = Num { val: 4 };
            n.double().val
        }
    "#,
    )
    .expect_output("40");
}

#[test]
fn test_inherent_impl_multiple_blocks_merge() {
    // Several impl blocks for one type merge; only duplicate method
    // names collide.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-000000000009) struct Point { x: Number, y: Number }

        impl Point {
            fn sum(self): Number {
                self.x + self.y
            }
        }

        impl Point {
            fn swap(self): Point {
                Point { x: self.y, y: self.x }
            }
        }

        fn run(): Number {
            let p = Point { x: 1, y: 41 };
            p.swap().sum()
        }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_inherent_impl_on_structural_type_error() {
    // Structural types have no identity to attach methods to.
    CliTest::new(
        r#"
        impl { x: Number } {
            fn get_x(self): Number {
                self.x
            }
        }

        fn run(): Number { 0 }
    "#,
    )
    .expect_failure();
}

#[test]
fn test_inherent_impl_missing_return_type_error() {
    // Inherent method signatures are the dispatch contract; the return
    // type must be declared.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-00000000000A) struct Money { cents: Number }

        impl Money {
            fn double(self) {
                Money { cents: self.cents * 2 }
            }
        }

        fn run(): Number { 0 }
    "#,
    )
    .expect_error("must declare a return type");
}

#[test]
fn test_multiple_traits_same_type() {
    // Test implementing multiple traits for the same type
    CliTest::new(
        r#"
        trait Doubler {
            fn double(self): Self;
        }

        trait Tripler {
            fn triple(self): Self;
        }

        unique(A1B2C3D4-0000-0000-0000-000000000005) struct Num { val: Number }

        impl Doubler for Num {
            fn double(self): Num {
                Num { val: self.val * 2 }
            }
        }

        impl Tripler for Num {
            fn triple(self): Num {
                Num { val: self.val * 3 }
            }
        }

        fn run(): Number {
            let n = Num { val: 5 };
            let doubled = n.double();
            let tripled = n.triple();
            doubled.val + tripled.val
        }
    "#,
    )
    .expect_output("25");
}

#[test]
fn test_impl_method_calls_top_level_function() {
    // Regression: impl methods are compiled through the same hash
    // finalization as ordinary functions, so calls from an impl method to a
    // top-level function must resolve at runtime. (Previously the call was
    // left as an unresolved temporary hash: UnknownFunction at runtime.)
    CliTest::new(
        r#"
        trait Show {
            fn show(self): Number;
        }

        unique(A1B2C3D4-0000-0000-0000-000000000006) struct Wrapper { value: Number }

        fn double(n: Number): Number { n * 2 }

        impl Show for Wrapper {
            fn show(self): Number {
                double(self.value)
            }
        }

        fn run(): Number {
            let w = Wrapper { value: 21 };
            w.show()
        }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_impl_method_with_lambda() {
    // Regression: lambdas inside impl methods must be compiled and linked.
    // (Previously impl methods used a throwaway module context, so their
    // lambdas were silently dropped.)
    CliTest::new(
        r#"
        trait Transform {
            fn apply(self): Number;
        }

        unique(A1B2C3D4-0000-0000-0000-000000000007) struct Box { value: Number }

        impl Transform for Box {
            fn apply(self): Number {
                let f = (x) => x + 1;
                f(self.value)
            }
        }

        fn run(): Number {
            let b = Box { value: 41 };
            b.apply()
        }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_operator_overloading_ne() {
    // `!=` must negate the Eq trait's `eq` result.
    CliTest::new(
        r#"
        trait Eq {
            fn eq(self, other: Self): Bool;
        }

        unique(A1B2C3D4-0000-0000-0000-000000000008) struct Id { value: Number }

        impl Eq for Id {
            fn eq(self, other: Id): Bool {
                self.value == other.value
            }
        }

        fn run(): Bool {
            let a = Id { value: 1 };
            let b = Id { value: 2 };
            a != b
        }
    "#,
    )
    .expect_output("true");
}

#[test]
fn test_operator_overloading_ordering() {
    // `<`, `<=`, `>`, `>=` must compare the Ord trait's `cmp` result
    // (-1/0/1) against zero rather than returning it directly.
    CliTest::new(
        r#"
        trait Ord {
            fn cmp(self, other: Self): Number;
        }

        unique(A1B2C3D4-0000-0000-0000-000000000009) struct Money { cents: Number }

        impl Ord for Money {
            fn cmp(self, other: Money): Number {
                if self.cents < other.cents { 0 - 1 } else {
                    if self.cents > other.cents { 1 } else { 0 }
                }
            }
        }

        fn run(): Number {
            let small = Money { cents: 50 };
            let big = Money { cents: 100 };

            let c1 = if small < big { 1 } else { 0 };
            let c2 = if big > small { 1 } else { 0 };
            let c3 = if small <= small { 1 } else { 0 };
            let c4 = if big >= small { 1 } else { 0 };
            let c5 = if big < small { 0 } else { 1 };
            c1 + c2 + c3 + c4 + c5
        }
    "#,
    )
    .expect_output("5");
}

// ─────────────────────────────────────────────────────────────────────────────
// Cross-module traits
// ─────────────────────────────────────────────────────────────────────────────

/// Create a temp package with multiple source files: (name, content) pairs,
/// where name is relative to src/ (e.g. "main.ab", "money.ab").
fn temp_multi_package(files: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("failed to create temp dir");
    let pkg_dir = dir.path().to_path_buf();

    fs::write(
        pkg_dir.join("ambient.toml"),
        r#"[package]
name = "test_pkg"
version = "0.1.0"

[build]
src = "src"
"#,
    )
    .expect("failed to write manifest");

    let src_dir = pkg_dir.join("src");
    fs::create_dir_all(&src_dir).expect("failed to create src dir");
    for (name, content) in files {
        fs::write(src_dir.join(name), content).expect("failed to write source file");
    }

    (dir, pkg_dir)
}

#[test]
fn test_cross_module_trait_dispatch() {
    // A type, its trait impls (using the prelude Add trait and a local
    // trait), and its constructor live in one module; another module calls
    // the operator and the method. Dispatch symbols must link across the
    // module boundary.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "money.ab",
            r#"
            pub unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00001111) struct Money { cents: Number }

            impl Add for Money {
                fn add(self, other: Money): Money {
                    Money { cents: self.cents + other.cents }
                }
            }

            pub trait Doubled {
                fn doubled(self): Number;
            }

            impl Doubled for Money {
                fn doubled(self): Number {
                    self.cents * 2
                }
            }

            pub fn make(cents: Number): Money {
                Money { cents: cents }
            }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::money::{Money, make};

            pub fn run(): Number {
                let total = make(100) + make(50);
                total.doubled() + total.cents
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("450"),
        "expected 450 in output, got: {stdout}"
    );
}

#[test]
fn test_cross_module_inherent_dispatch() {
    // Inherent methods link across module boundaries exactly like trait
    // methods: the dispatch symbol resolves by type identity, no import
    // of the impl needed.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "money.ab",
            r#"
            pub unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00003333) struct Money { cents: Number }

            impl Money {
                fn doubled(self): Number {
                    self.cents * 2
                }
                fn zero(): Money {
                    Money { cents: 0 }
                }
            }

            pub fn make(cents: Number): Money {
                Money { cents: cents }
            }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::money::{Money, make};

            pub fn run(): Number {
                let m = make(100);
                m.doubled() + Money::zero().cents
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("200"),
        "expected 200 in output, got: {stdout}"
    );
}

#[test]
fn test_cross_module_enum_import() {
    // Importing an enum brings its type, variant constructors, and
    // patterns into scope, exactly as if it were declared locally.
    // Inherent methods dispatch by uuid, so they need no import at all.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "shapes.ab",
            r#"
            pub unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00002222) enum Shape {
                Circle(Number),
                Square(Number),
                Dot,
            }

            impl Shape {
                fn area(self): Number {
                    match self {
                        Circle(r) => 3 * r * r,
                        Square(side) => side * side,
                        Dot => 0,
                    }
                }
            }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::shapes::{Shape};

            fn describe(s: Shape): Number {
                match s {
                    Circle(r) => r,
                    Square(side) => side * 2,
                    Dot => 100,
                }
            }

            pub fn run(): Number {
                describe(Circle(10)) + describe(Dot) + Circle(2).area() + Square(3).area()
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // 10 + 100 + 12 + 9
    assert!(
        stdout.contains("131"),
        "expected 131 in output, got: {stdout}"
    );
}

#[test]
fn test_cross_module_const_import() {
    // A `pub const` imported into another module inlines its literal value
    // at each reference, exactly as if it were declared locally. The value
    // travels the same AST channel as imported enums, not a hash link.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "config.ab",
            r#"
            pub const ANSWER: Number = 42;
            pub const SCALE: Number = 100;
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::config::{ANSWER, SCALE};

            pub fn run(): Number {
                ANSWER + SCALE
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // 42 + 100
    assert!(
        stdout.contains("142"),
        "expected 142 in output, got: {stdout}"
    );
}

#[test]
fn test_enum_variant_import_is_rejected() {
    // Variants don't import piecemeal — patterns and constructor tags
    // need the whole declaration in scope.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "shapes.ab",
            r#"
            pub unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00005555) enum Shape {
                Circle(Number),
                Dot,
            }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::shapes::{Circle};

            pub fn run(): Number {
                0
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    assert!(!output.status.success(), "expected variant import to fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("import its enum"),
        "expected variant-import hint, got: {stderr}"
    );
}

#[test]
fn test_foreign_nominal_type_hidden_without_import() {
    // A foreign package type is not visible by bare name unless imported:
    // constructing it without a `use` is an error, even though its module
    // is part of the same package. Trait/impl coherence stays build-global,
    // but nominal types follow the same `pub`/`use` rules as values.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "widgets.ab",
            r#"
            pub unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00004444) struct Widget { size: Number }
            "#,
        ),
        (
            "main.ab",
            r#"
            pub fn run(): Number {
                Widget { size: 42 }.size
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    assert!(
        !output.status.success(),
        "constructing an unimported foreign type must fail, stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn test_foreign_nominal_type_visible_with_import() {
    // Importing the type by name makes it constructible, just like a local
    // declaration — the `use` is what brings it into scope.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "widgets.ab",
            r#"
            pub unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00005544) struct Widget { size: Number }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::widgets::{Widget};

            pub fn run(): Number {
                Widget { size: 42 }.size
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("42"),
        "expected 42 in output, got: {stdout}"
    );
}

#[test]
fn test_imported_functions_share_foreign_nominal_identity() {
    // A value of a foreign type flows between two imported functions
    // without the caller ever naming the type: signature hydration still
    // resolves the nominal identity even though the type stays hidden from
    // bare-name use. This guards the retraction that closes the leak.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "widgets.ab",
            r#"
            pub unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00006644) struct Widget { size: Number }

            pub fn make(n: Number): Widget { Widget { size: n } }
            pub fn size_of(w: Widget): Number { w.size }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::widgets::{make, size_of};

            pub fn run(): Number {
                size_of(make(42))
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("42"),
        "expected 42 in output, got: {stdout}"
    );
}

#[test]
fn test_reserved_uuid_cannot_be_hijacked() {
    // Option's reserved uuid with a different shape must be rejected —
    // otherwise the declaration would unify with real Options and claim
    // their inherent methods.
    let (_dir, pkg) = temp_multi_package(&[(
        "main.ab",
        r#"
        unique(FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFF0001) enum MyOption<T> {
            Nothing,
            Just(T),
        }

        pub fn run(): Number {
            0
        }
        "#,
    )]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    assert!(!output.status.success(), "expected hijack to be rejected");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("reserved identity"),
        "expected reserved-identity error, got: {stderr}"
    );
}

#[test]
fn test_private_enum_is_not_importable() {
    // A bare `enum` (no `pub`) stays module-local.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "shapes.ab",
            r#"
            unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00006666) enum Secret {
                Hidden,
            }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::shapes::{Secret};

            pub fn run(): Number {
                0
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    assert!(!output.status.success(), "expected private import to fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not public"),
        "expected visibility error, got: {stderr}"
    );
}

#[test]
fn test_cross_module_duplicate_inherent_method_error() {
    // Two modules in the build closure defining the same inherent method
    // for the same type is unresolvable ambiguity: both definitions claim
    // one dispatch symbol. (Coherence is scoped to the build closure —
    // modules never loaded into a program can't collide with it.)
    let (_dir, pkg) = temp_multi_package(&[
        (
            "a.ab",
            r#"
            pub unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00004444) struct Money { cents: Number }

            impl Money {
                fn doubled(self): Number { self.cents * 2 }
            }

            pub fn make(cents: Number): Money {
                Money { cents: cents }
            }
            "#,
        ),
        (
            "b.ab",
            r#"
            use pkg::a::{Money};

            impl Money {
                fn doubled(self): Number { self.cents * 4 }
            }

            pub fn touch(): Number { 1 }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::a::{make};
            use pkg::b::{touch};

            pub fn run(): Number {
                make(touch()).doubled()
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    assert!(
        !output.status.success(),
        "duplicate cross-module inherent methods must be rejected"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("duplicate inherent method"),
        "expected duplicate inherent method error, got: {stderr}"
    );
}

#[test]
fn test_core_option_and_list_methods() {
    // The core library's Option/Result/List helpers are exposed as real
    // methods via inherent impls written in Ambient (core_lib/*.ab).
    CliTest::new(
        r#"
        pub fn run(): Number {
            let doubled = Some(20).map((v) => v * 2).unwrap_or(0);
            let empty = None.unwrap_or(2);
            let list_sum = [1, 2, 3].map((x) => x * 10).fold(0, (acc, x) => acc + x);
            let evens = [1, 2, 3, 4].filter((x) => x % 2 == 0).length();
            let chained = Ok(5).map((v) => v + 1).ok().unwrap_or(0);
            doubled + empty + list_sum + evens + chained
        }
    "#,
    )
    .expect_output("110");
}

#[test]
fn test_core_method_and_module_call_coexist() {
    // A module companion function `scaled(n, factor)` (called qualified as
    // `nums::scaled(...)`) and an inherent method `.scaled(factor)` of the
    // same name both resolve and agree. Core no longer exposes combinators
    // as free functions, so the coexistence is demonstrated with a user
    // module rather than `core::Option::map`.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "nums.ab",
            r"
            pub fn scaled(n: Number, factor: Number): Number { n * factor }
            ",
        ),
        (
            "main.ab",
            r"
            use self::nums;

            unique(BBBBCCCC-DDDD-EEEE-FFFF-000011112222) struct Counter { value: Number }

            impl Counter {
                fn scaled(self, factor: Number): Number { self.value * factor }
            }

            pub fn run(): Bool {
                let via_module = nums::scaled(5, 2);
                let via_method = Counter { value: 5 }.scaled(2);
                via_module == via_method
            }
            ",
        ),
    ]);
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("true"));
}

#[test]
fn test_user_cannot_redefine_core_method() {
    // Core already defines `map` for Option; a user redefinition would
    // compete for the same dispatch symbol and is rejected.
    CliTest::new(
        r#"
        impl<T> Option<T> {
            fn map<U>(self, f: (T) -> U): Option<U> {
                None
            }
        }

        fn run(): Number { 0 }
    "#,
    )
    .expect_error("duplicate inherent method");
}

#[test]
fn test_inherent_impl_on_primitives() {
    // Primitives carry inherent methods too — their type identity is the
    // reserved lowercase head no user type can claim.
    CliTest::new(
        r#"
        impl String {
            fn shout(self): String {
                core::primitives::String::to_upper(self)
            }
        }

        impl Number {
            fn clamped(self, lo: Number, hi: Number): Number {
                core::primitives::Number::min(core::primitives::Number::max(self, lo), hi)
            }
        }

        pub fn run(): String {
            "hi ${(99).clamped(0, 42)} " + "there".shout()
        }
    "#,
    )
    .expect_output("hi 42 THERE");
}

#[test]
fn test_string_concat_operator_at_runtime() {
    // `+` on two strings concatenates (the checker has always admitted
    // this; the VM used to reject it at runtime).
    CliTest::new(
        r#"
        pub fn run(): String {
            let name = "world";
            "hello " + name + "!"
        }
    "#,
    )
    .expect_output("hello world!");
}

#[test]
fn test_user_can_extend_core_type_with_new_method() {
    // New method names on core types are fair game — extension without
    // collision.
    CliTest::new(
        r#"
        impl<T> Option<T> {
            fn to_list(self): List<T> {
                match self {
                    Some(v) => [v],
                    None => [],
                }
            }
        }

        pub fn run(): Number {
            Some(7).to_list().length() + None.to_list().length()
        }
    "#,
    )
    .expect_output("1");
}

#[test]
fn test_prelude_traits_no_import_needed() {
    // The operator traits are prelude: an impl can reference Add without
    // any use statement or local trait declaration.
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00002222) struct Meters { value: Number }

        impl Add for Meters {
            fn add(self, other: Meters): Meters {
                Meters { value: self.value + other.value }
            }
        }

        fn run(): Number {
            let d = Meters { value: 3 } + Meters { value: 4 };
            d.value
        }
    "#,
    )
    .expect_output("7");
}

#[test]
fn test_zero_parameter_lambda() {
    // `()` must parse as a unit literal except when followed by `=>`,
    // where it begins a zero-parameter lambda.
    CliTest::new(
        r#"
        fn call_thunk(f: () -> Number): Number {
            f()
        }

        fn run(): Number {
            let t = () => 42;
            let a = t();
            let b = call_thunk(() => { let x = 7; x * 2 });
            let unit_still_works = ();
            a + b
        }
    "#,
    )
    .expect_output("56");
}

// ─────────────────────────────────────────────────────────────────────────────
// Ability inference for private functions
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_private_function_ability_inference() {
    // Private helpers need no `with` annotations: their abilities are
    // inferred from their bodies (including through mutual recursion and
    // calls to functions defined later) and propagate to callers.
    CliTest::new(
        r#"
        pub fn run(): () with core::system::Stdio {
            ping(2);
            helper_outer();
        }

        fn ping(n: Number) {
            if n > 0 { core::system::Stdio::out!("ping"); pong(n - 1); } else { () }
        }

        fn pong(n: Number) {
            if n > 0 { core::system::Stdio::out!("pong"); ping(n - 1); } else { () }
        }

        fn helper_inner() { core::system::Stdio::out!("inner"); }
        fn helper_outer() { helper_inner(); }
    "#,
    )
    .expect_output("ping");
}

#[test]
fn test_public_function_must_declare_inferred_abilities() {
    // Inferred abilities from private helpers still count against a public
    // function's declarations — declaring pure while transitively performing
    // Stdio is an error, even when the helper is defined after the caller.
    CliTest::new(
        r#"
        pub fn run(): () {
            leaky();
        }

        fn leaky() {
            core::system::Stdio::out!("leak");
        }
    "#,
    )
    .expect_error("uses ability `Stdio` but doesn't declare it");
}

#[test]
fn test_duplicate_impl_is_error() {
    // A trait may be implemented at most once per type: the impl-method
    // dispatch symbol is derived from (type uuid, trait, method), so a
    // second impl would collide in the content-addressed store.
    CliTest::new(
        r#"
        trait Show {
            fn show(self): Number;
        }

        unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00003333) struct Id { value: Number }

        impl Show for Id {
            fn show(self): Number { self.value }
        }

        impl Show for Id {
            fn show(self): Number { self.value * 2 }
        }

        fn run(): Number {
            let i = Id { value: 1 };
            i.show()
        }
    "#,
    )
    .expect_error("duplicate implementation of trait `Show`");
}

#[test]
fn test_ambiguous_method_is_error() {
    // Two different traits implemented for the same type both provide a
    // method named `render`; a bare method call cannot choose between them.
    CliTest::new(
        r#"
        trait Html {
            fn render(self): Number;
        }

        trait Text {
            fn render(self): Number;
        }

        unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00004444) struct Page { id: Number }

        impl Html for Page {
            fn render(self): Number { self.id }
        }

        impl Text for Page {
            fn render(self): Number { self.id * 2 }
        }

        fn run(): Number {
            let p = Page { id: 1 };
            p.render()
        }
    "#,
    )
    .expect_error("render");
}

// ─────────────────────────────────────────────────────────────────────────────
// Honest Partial Intrinsics (Option-returning accessors)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_list_accessors_return_option() {
    // `List::get`/`head`/`first`/`last` return Option: `Some` on a hit,
    // `None` when the element is missing — never a substituted `()`.
    let (dir, pkg) = temp_package(
        r#"
        pub fn run(): String {
            let hit = match core::collections::List::get([1, 2, 3], 1) {
                Some(v) => v,
                None => 0 - 1,
            };
            let miss = match core::collections::List::head([]) {
                Some(v) => v,
                None => 0 - 1,
            };
            let first = core::collections::List::first([7, 8]).unwrap_or(0);
            let last = core::collections::List::last([7, 8]).unwrap_or(0);
            core::convert::to_string(hit) + " " + core::convert::to_string(miss)
                + " " + core::convert::to_string(first) + " " + core::convert::to_string(last)
        }
        "#,
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("2 -1 7 8"));
    drop(dir);
}

#[test]
fn test_list_option_chains_through_method_combinators() {
    // The Option flows through the inherent method forms:
    // `xs.head().unwrap_or(...)`, `xs.get(i).map(...)`.
    let (dir, pkg) = temp_package(
        r"
        pub fn run(): Number {
            let xs = [10, 20, 30];
            let empty: List<Number> = [];
            xs.head().unwrap_or(0)
                + xs.get(2).map((v: Number) => v * 2).unwrap_or(0)
                + xs.last().unwrap_or(0)
                + empty.head().unwrap_or(1000)
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    // 10 + 60 + 30 + 1000
    assert!(String::from_utf8_lossy(&output.stdout).contains("1100"));
    drop(dir);
}

#[test]
fn test_parse_number_and_parse_bool_return_option() {
    let (dir, pkg) = temp_package(
        r#"
        pub fn run(): String {
            let good = core::convert::parse_number("42.5").unwrap_or(0);
            let bad = match core::convert::parse_number("abc") {
                Some(_) => "some",
                None => "none",
            };
            let yes = core::convert::parse_bool("true").unwrap_or(false);
            let junk = core::convert::parse_bool("maybe").is_none();
            core::convert::to_string(good) + " " + bad + " "
                + core::convert::to_string(yes) + " " + core::convert::to_string(junk)
        }
        "#,
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("42.5 none true true"));
    drop(dir);
}

#[test]
fn test_map_get_returns_option() {
    let (dir, pkg) = temp_package(
        r#"
        pub fn run(): String {
            let m = core::collections::map::insert(core::collections::map::empty(), "a", 1);
            let hit = core::collections::map::get(m, "a").unwrap_or(0);
            let miss = match core::collections::map::get(m, "b") {
                Some(_) => "some",
                None => "none",
            };
            core::convert::to_string(hit) + " " + miss
        }
        "#,
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 none"));
    drop(dir);
}

#[test]
fn test_string_index_of_returns_option() {
    let (dir, pkg) = temp_package(
        r#"
        pub fn run(): String {
            let found = core::primitives::String::index_of("hello world", "wor").unwrap_or(0 - 1);
            let missing = core::primitives::String::index_of("hello", "xyz").is_none();
            core::convert::to_string(found) + " " + core::convert::to_string(missing)
        }
        "#,
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("6 true"));
    drop(dir);
}

// ─────────────────────────────────────────────────────────────────────────────
// Module System Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_core_functions_fully_qualified() {
    // Compiled core functions (not intrinsics) callable with no import.
    // `range` is the surviving compiled core free function — combinators are
    // now methods, so the chain uses `.sum()`.
    let (dir, pkg) = temp_package(
        r"
        pub fn run(): Number {
            core::collections::List::range(1, 5).sum()
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("10"));
    drop(dir);
}

#[test]
fn test_core_whole_module_import_alias() {
    // `use core::collections::List;` binds the alias `List` for qualified calls.
    let (dir, pkg) = temp_package(
        r"
        use core::collections::List;

        pub fn run(): Number {
            List::range(1, 5).fold(0, (acc: Number, x: Number) => acc + x)
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("10"));
    drop(dir);
}

#[test]
fn test_core_item_import() {
    // `use core::collections::List::{range};` binds a plain name. (Combinators are now
    // methods, so `range` — a receiverless core free function — is the
    // importable plain name; the chain finishes with the `.sum()` method.)
    let (dir, pkg) = temp_package(
        r"
        use core::collections::List::{range};

        pub fn run(): Number {
            range(1, 5).sum()
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("10"));
    drop(dir);
}

#[test]
fn test_non_brace_item_import() {
    // `use pkg::utils::triple;` (no braces) imports the *item* `triple`,
    // exactly like the brace form. Braces are pure grouping.
    let (dir, pkg) = temp_multi_package(&[
        (
            "main.ab",
            r"
            use pkg::utils::triple;

            pub fn run(): Number {
                triple(7) + triple(1)
            }
            ",
        ),
        (
            "utils.ab",
            r"
            pub fn triple(x: Number): Number { x * 3 }
            ",
        ),
    ]);
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("24"));
    drop(dir);
}

#[test]
fn test_non_brace_core_item_import() {
    // The unification reaches core too: `use core::collections::List::range;` binds the
    // bare name `range` without braces.
    let (dir, pkg) = temp_package(
        r"
        use core::collections::List::range;

        pub fn run(): Number {
            range(1, 4).sum()
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("6"));
    drop(dir);
}

#[test]
fn test_whole_module_user_import() {
    // `use self::utils;` then `utils::helper()` — qualified module-member
    // calls on user modules.
    let (dir, pkg) = temp_multi_package(&[
        (
            "main.ab",
            r"
            use self::utils;

            pub fn run(): Number {
                utils::triple(7) + utils::triple(1)
            }
            ",
        ),
        (
            "utils.ab",
            r"
            pub fn triple(x: Number): Number { x * 3 }
            ",
        ),
    ]);
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("24"));
    drop(dir);
}

#[test]
fn test_local_variable_shadows_module_alias() {
    // A local binding named like a module alias wins: `utils.triple` is
    // then a (failing) trait-method call on the value, not a module call.
    let (dir, pkg) = temp_multi_package(&[
        (
            "main.ab",
            r"
            use self::utils;

            pub fn run(): Number {
                let utils = 5;
                utils.triple(7)
            }
            ",
        ),
        (
            "utils.ab",
            r"
            pub fn triple(x: Number): Number { x * 3 }
            ",
        ),
    ]);
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(
        !output.status.success(),
        "shadowed alias must not resolve as a module call: {output:?}"
    );
    drop(dir);
}

#[test]
fn test_method_call_resolves_inside_perform_arguments() {
    // Regression: perform arguments used to be type-checked on
    // CLONES of the argument expressions, so resolutions recorded during
    // inference (trait method symbols, operator overloads) were silently
    // discarded and compilation failed.
    let (dir, pkg) = temp_package(
        r#"
        unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00001111) struct Point { x: Number }

        trait Doubled {
            fn doubled(self): Number;
        }

        impl Doubled for Point {
            fn doubled(self): Number { self.x * 2 }
        }

        pub fn run(): () with core::system::Stdio {
            let p = Point { x: 21 };
            core::system::Stdio::out!(core::convert::to_string(p.doubled()));
        }
        "#,
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("42"));
    drop(dir);
}

// ─────────────────────────────────────────────────────────────────────────────
// Enum Constructor & Match Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_user_enum_construct_and_match() {
    let (dir, pkg) = temp_package(
        r"
        unique(C1B2C3D4-0000-0000-0000-000000000011) enum Shape { Circle(Number), Square(Number), Dot }

        pub fn run(): Number {
            area(Circle(2)) + area(Square(3)) + area(Dot)
        }

        fn area(s: Shape): Number {
            match s {
                Circle(r) => 3 * r * r,
                Square(side) => side * side,
                Dot => 0,
            }
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("21"));
    drop(dir);
}

#[test]
fn test_bare_enum_requires_unique() {
    // Every enum must carry a `unique(<uuid>)` prefix; a bare `enum` is
    // rejected so structurally identical enums can never be conflated.
    CliTest::new(
        r"
        enum Color { Red, Green, Blue }

        pub fn run(): Number { 0 }
        ",
    )
    .check()
    .expect_error("unique");
}

#[test]
fn test_distinct_enums_are_not_interchangeable() {
    // Two enums with identical shape are distinct nominal types: a value of
    // one cannot stand in for the other. This is the whole point of enum
    // nominal identity — shape no longer implies interchangeability.
    CliTest::new(
        r"
        unique(C1B2C3D4-0000-0000-0000-000000000020) enum Meters { M(Number) }
        unique(C1B2C3D4-0000-0000-0000-000000000021) enum Feet { F(Number) }

        fn meters_value(x: Meters): Number {
            match x { M(v) => v }
        }

        pub fn run(): Number {
            meters_value(F(3))
        }
        ",
    )
    .check()
    .expect_error("type mismatch");
}

#[test]
fn test_duplicate_inherent_method_on_enum_error() {
    // Coherence holds for enums exactly as for nominal types: a second
    // definition of a method name for the same enum is rejected because both
    // would claim the one `<uuid>::method` dispatch symbol.
    CliTest::new(
        r#"
        unique(C1B2C3D4-0000-0000-0000-000000000022) enum Toggle { On, Off }

        impl Toggle {
            fn flipped(self): Toggle {
                match self { On => Off, Off => On }
            }
        }

        impl Toggle {
            fn flipped(self): Toggle {
                self
            }
        }

        pub fn run(): Number { 0 }
    "#,
    )
    .check()
    .expect_error("duplicate inherent method");
}

#[test]
fn test_generic_nominal_enum_roundtrips() {
    // A generic `unique(...) enum` carries its type argument through
    // construction, matching, and an inherent method that returns the
    // payload — proving the nominal identity survives substitution.
    let (dir, pkg) = temp_package(
        r"
        unique(C1B2C3D4-0000-0000-0000-000000000023) enum Box<T> { Full(T), Empty }

        impl<T> Box<T> {
            fn get_or(self, fallback: T): T {
                match self {
                    Full(v) => v,
                    Empty => fallback,
                }
            }
        }

        pub fn run(): Number {
            Full(40).get_or(0) + Empty.get_or(2)
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("42"));
    drop(dir);
}

#[test]
fn test_enum_payload_is_another_nominal_enum() {
    // A variant payload written as another declared enum resolves to that
    // enum's nominal identity, so a method call on the extracted binding
    // dispatches on the payload enum's uuid — not its head name.
    let (dir, pkg) = temp_package(
        r"
        unique(D1B2C3D4-0000-0000-0000-000000000001) enum Inner { Val(Number) }
        unique(D1B2C3D4-0000-0000-0000-000000000002) enum Outer { Wrap(Inner) }

        impl Inner {
            fn doubled(self): Number {
                match self { Val(v) => v * 2 }
            }
        }

        pub fn run(): Number {
            match Wrap(Val(21)) {
                Wrap(inner) => inner.doubled(),
            }
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("42"));
    drop(dir);
}

#[test]
fn test_option_constructors_and_core_helpers() {
    let (dir, pkg) = temp_package(
        r"
        pub fn run(): Number {
            let doubled = Some(20).map((x: Number) => x * 2);
            doubled.unwrap_or(0)
                + nothing().map((x: Number) => x).unwrap_or(2)
        }

        fn nothing(): Option<Number> {
            None
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("42"));
    drop(dir);
}

#[test]
fn test_result_constructors_and_chaining() {
    let (dir, pkg) = temp_package(
        r#"
        pub fn run(): String {
            let ok = parse(5).map((x: Number) => x * 10);
            let err = parse(0 - 3);
            match ok.and_then((x: Number) => parse(x)) {
                Ok(v) => core::primitives::String::from_number(v),
                Err(e) => e,
            }
        }

        fn parse(n: Number): Result<Number, String> {
            if n > 0 { Ok(n) } else { Err("negative") }
        }
        "#,
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("50"));
    drop(dir);
}

#[test]
fn test_match_takes_correct_arm() {
    // Regression: the pattern compiler's success path used to jump straight
    // to the fail target, so every variant arm skipped its own body.
    let (dir, pkg) = temp_package(
        r"
        pub fn run(): Number {
            let hit = match Some(41) {
                Some(v) => v,
                None => 0 - 1,
            };
            let miss = match nothing() {
                Some(v) => v,
                None => 100,
            };
            hit + miss
        }

        fn nothing(): Option<Number> {
            None
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("141"));
    drop(dir);
}

#[test]
fn test_unknown_variant_pattern_is_error() {
    let (dir, pkg) = temp_package(
        r"
        pub fn run(): Number {
            match Some(1) {
                Sume(v) => v,
                None => 0,
            }
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown enum variant"),
        "expected unknown-variant error: {output:?}"
    );
    drop(dir);
}

#[test]
fn test_variant_payload_mismatch_is_error() {
    let (dir, pkg) = temp_package(
        r"
        pub fn run(): Number {
            match Some(1) {
                Some => 1,
                None => 0,
            }
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("payload"),
        "expected payload mismatch error: {output:?}"
    );
    drop(dir);
}

#[test]
fn test_lowercase_pattern_still_binds() {
    // Only uppercase-initial bare identifiers are variant patterns.
    let (dir, pkg) = temp_package(
        r"
        pub fn run(): Number {
            match 42 {
                x => x,
            }
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("42"));
    drop(dir);
}

// ─────────────────────────────────────────────────────────────────────────────
// User-Declared Abilities (content-addressed)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_user_ability_inline_handler() {
    CliTest::new(
        r#"
        ability Greeter {
            fn greet(name: String): String;
        }

        fn hello(): String with Greeter {
            Greeter::greet!("world")
        }

        pub fn run(): String {
            with {
                Greeter::greet(name) => {
                    resume(core::primitives::String::concat("hi ", name))
                }
            } handle hello()
        }
        "#,
    )
    .expect_output("hi world");
}

#[test]
fn test_user_ability_handler_value_and_generic_method() {
    // A handler value (first-class) for a user ability with a generic
    // method. Also a regression test: `handle ... with value {}` used to
    // silently install no handler because inference typed handler values
    // on cloned AST nodes.
    CliTest::new(
        r#"
        ability Picker {
            fn pick<T>(a: T, b: T): T;
            fn label(): String;
        }

        fn choose(): Number with Picker {
            Picker::pick!(10, 32)
        }

        pub fn run(): Number {
            let first = {
                Picker::pick(a, b) => resume(a),
                Picker::label() => resume("first")
            };

            with first handle choose()
        }
        "#,
    )
    .expect_output("10");
}

#[test]
fn test_user_ability_unhandled_is_runtime_error() {
    CliTest::new(
        r#"
        ability Missing {
            fn gone(): String;
        }

        pub fn run(): String with Missing {
            Missing::gone!()
        }
        "#,
    )
    .expect_error("unhandled ability");
}

#[test]
fn test_user_ability_unknown_method_is_type_error() {
    CliTest::new(
        r#"
        ability Greeter {
            fn greet(name: String): String;
        }

        pub fn run(): String with Greeter {
            Greeter::shout!("hi")
        }
        "#,
    )
    .expect_failure();
}

#[test]
fn test_user_ability_wrong_arg_type_is_type_error() {
    CliTest::new(
        r#"
        ability Greeter {
            fn greet(name: String): String;
        }

        pub fn run(): String with Greeter {
            Greeter::greet!(42)
        }
        "#,
    )
    .expect_failure();
}

#[test]
fn test_user_ability_unknown_dependency_is_error() {
    CliTest::new(
        r"
        ability Loud with NoSuchAbility {
            fn shout(msg: String): ();
        }

        pub fn run(): Number {
            7
        }
        ",
    )
    .expect_failure();
}

#[test]
fn test_suspend_form_is_removed() {
    // The `~` suspend-call syntax was removed from the language; using it
    // is now a parse error.
    CliTest::new(
        r#"
        ability Greeter {
            fn greet(name: String): String;
        }

        pub fn run(): Number {
            let op = Greeter::greet~("later");
            7
        }
        "#,
    )
    .expect_failure();
}

// ─────────────────────────────────────────────────────────────────────────────
// Remote Execution with Ability Dispatch
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_execute_run_with_granted_ability() {
    // Execute.run runs code in an isolated VM. The host grants output
    // abilities (Stdio/Log) to executed code, so an effectful function
    // can be run by hash and its logs land on the executing host.
    CliTest::new(
        r#"
        fn shout(x: Number): Number with core::system::Log, core::system::Stdio {
            core::system::Log::info!("computing remotely");
            x * 2
        }

        pub fn run(): Number with core::system::Execute {
            let thunk = (x) => shout(x);
            let hash = core::protocol::closure_hash(thunk);
            core::system::Execute::run!(hash, 21)
        }
        "#,
    )
    .expect_output("42");
}

#[test]
fn test_execute_run_ungranted_ability_is_unhandled() {
    // Network is NOT granted to executed code: performing it inside an
    // isolated VM is an unhandled-ability error, not a silent escape.
    CliTest::new(
        r#"
        fn phone_home(x: Number): Number with core::system::Network {
            let conn = core::system::Network::connect!(("127.0.0.1", 1));
            x
        }

        pub fn run(): Number with core::system::Execute {
            let thunk = (x) => phone_home(x);
            let hash = core::protocol::closure_hash(thunk);
            core::system::Execute::run!(hash, 1)
        }
        "#,
    )
    .expect_error("unhandled ability");
}

#[test]
fn test_execute_run_with_shipped_handler() {
    // The flagship composition: a user-declared (content-addressed)
    // ability, a first-class handler value whose methods are
    // content-addressed functions, and Execute.run_with installing that
    // handler at the base of the isolated VM. The shipped code performs
    // the ability; the shipped handler answers it.
    CliTest::new(
        r"
        ability Oracle {
            fn answer(): Number;
        }

        fn consult(x: Number): Number with Oracle {
            x + Oracle::answer!()
        }

        pub fn run(): Number with core::system::Execute {
            let oracle = { Oracle::answer() => resume(40) };
            let thunk = (x) => consult(x);
            let hash = core::protocol::closure_hash(thunk);
            core::system::Execute::run_with!(hash, 2, oracle)
        }
        ",
    )
    .expect_output("42");
}

#[test]
fn test_handler_methods_intrinsic() {
    // handler_methods exposes a handler's content-addressed method
    // hashes so clients can ship the handler's code alongside a function.
    CliTest::new(
        r"
        ability Oracle {
            fn answer(): Number;
        }

        pub fn run(): Number {
            let oracle = { Oracle::answer() => resume(42) };
            core::collections::List::length(core::protocol::handler_methods(oracle))
        }
        ",
    )
    .expect_output("1");
}

// ─────────────────────────────────────────────────────────────────────────────
// Delimited handler semantics (catch-and-continue, resume, else)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_handle_catch_and_continue() {
    // A non-resuming arm's value becomes the handle expression's value,
    // and execution continues after the handle expression. This is the
    // essential try/catch shape.
    CliTest::new(
        r#"
        fn risky(): Number with Exception {
            Exception::throw!("kaboom");
            1
        }

        pub fn run(): Number {
            let caught = with {
                Exception::throw(msg) => 0 - 1
            } handle risky();
            caught + 100
        }
        "#,
    )
    .expect_output("99");
}

#[test]
fn test_resume_restores_locals() {
    // Locals bound before the perform must be intact after resume.
    // Regression test: continuations used to be captured with absolute
    // base pointers, so the resumed frames read the wrong stack slots.
    CliTest::new(
        r#"
        ability Oracle {
            fn ask(q: String): Number;
        }

        fn asker(): Number with Oracle {
            let base = 100;
            let answer = Oracle::ask!("q");
            base + answer
        }

        pub fn run(): Number {
            with {
                Oracle::ask(q) => resume(42)
            } handle asker()
        }
        "#,
    )
    .expect_output("142");
}

#[test]
fn test_handle_multi_perform_with_capturing_arm() {
    // Deep handler semantics: the handler stays installed across resumes,
    // so a body performing three times fires the same arm three times.
    // The arm also captures a local from the enclosing scope.
    CliTest::new(
        r"
        ability Counter {
            fn next(): Number;
        }

        fn count_three(): Number with Counter {
            let a = Counter::next!();
            let b = Counter::next!();
            let c = Counter::next!();
            a + b + c
        }

        pub fn run(): Number {
            let step = 10;
            with {
                Counter::next() => resume(step)
            } handle count_three()
        }
        ",
    )
    .expect_output("30");
}

#[test]
fn test_handle_else_transforms_normal_completion() {
    // The else clause transforms the body's value on normal completion;
    // handler arms bypass it.
    CliTest::new(
        r"
        pub fn run(): Number {
            with {
                Exception::throw(msg) => 0
            } handle 5 else (r) => r * 2
        }
        ",
    )
    .expect_output("10");
}

#[test]
fn test_exception_unwinds_through_inner_handle() {
    // A throw crosses an inner (non-Exception) handler region to reach
    // the outer Exception handler, and the inner handler is fully
    // uninstalled afterwards.
    CliTest::new(
        r#"
        ability Ping {
            fn ping(): Number;
        }

        fn inner(): Number with Ping, Exception {
            let p = Ping::ping!();
            Exception::throw!("escape");
            p
        }

        fn middle(): Number with Exception {
            with {
                Ping::ping() => resume(7)
            } handle inner()
        }

        pub fn run(): Number {
            let x = with {
                Exception::throw(msg) => 50
            } handle middle();
            let y = with {
                Ping::ping() => resume(1),
                Exception::throw(msg) => 2
            } handle inner();
            x + y
        }
        "#,
    )
    .expect_output("52");
}

#[test]
fn test_uncaught_exception_reports_value() {
    // With no handler in scope, the thrown value surfaces in the error.
    let output = CliTest::new(
        r#"
        pub fn run(): Number with Exception {
            Exception::throw!("boom with value 7");
            0
        }
        "#,
    )
    .execute();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("uncaught exception") && stderr.contains("boom with value 7"),
        "expected uncaught exception with thrown value, got: {stderr}"
    );
}

#[test]
fn test_host_raised_exception_is_catchable() {
    // A failing host operation (network connect to a closed port) raises
    // a catchable exception instead of aborting the VM.
    CliTest::new(
        r#"
        fn try_connect(): String with core::system::Network {
            let conn = core::system::Network::connect!(("127.0.0.1", 9));
            "connected"
        }

        pub fn run(): String with core::system::Network {
            with {
                Exception::throw(msg) => "failed"
            } handle try_connect()
        }
        "#,
    )
    .expect_output("failed");
}

#[test]
fn test_host_raised_exception_resume_substitute() {
    // The Exception handler receives the continuation of the failed host
    // call, so it can resume with a substitute value: try_connect
    // continues executing after the failed connect.
    CliTest::new(
        r#"
        fn try_connect(): Number with core::system::Network {
            let conn = core::system::Network::connect!(("127.0.0.1", 9));
            conn + 1000
        }

        pub fn run(): Number with core::system::Network {
            with {
                Exception::throw(msg) => resume(0 - 1)
            } handle try_connect()
        }
        "#,
    )
    .expect_output("999");
}

// ─────────────────────────────────────────────────────────────────────────────
// FileSystem Ability Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_fs_write_read_roundtrip() {
    let dir = TempDir::new().expect("failed to create temp dir");
    let path = dir.path().join("note.txt");
    CliTest::new(format!(
        r#"
        pub fn run(): String with core::system::FileSystem {{
            core::system::FileSystem::write!("{path}", "hello from ambient");
            core::system::FileSystem::read!("{path}")
        }}
        "#,
        path = path.display()
    ))
    .expect_output("hello from ambient");
}

#[test]
fn test_fs_read_missing_file_is_catchable_exception() {
    // A failing filesystem operation raises a catchable exception instead
    // of aborting the VM.
    CliTest::new(
        r#"
        fn try_read(): String with core::system::FileSystem {
            core::system::FileSystem::read!("/nonexistent/ambient_fs_test/missing.txt")
        }

        pub fn run(): String with core::system::FileSystem {
            with {
                Exception::throw(msg) => "caught"
            } handle try_read()
        }
        "#,
    )
    .expect_output("caught");
}

#[test]
fn test_fs_exists_false_then_true() {
    let dir = TempDir::new().expect("failed to create temp dir");
    let path = dir.path().join("probe.txt");
    CliTest::new(format!(
        r#"
        pub fn run(): () with core::system::FileSystem, core::system::Stdio {{
            core::system::Stdio::out!(core::convert::to_string(core::system::FileSystem::exists!("{path}")));
            core::system::FileSystem::write!("{path}", "x");
            core::system::Stdio::out!(core::convert::to_string(core::system::FileSystem::exists!("{path}")));
        }}
        "#,
        path = path.display()
    ))
    .expect_output("false\ntrue");
}

#[test]
fn test_fs_list_returns_written_entries() {
    let dir = TempDir::new().expect("failed to create temp dir");
    let base = dir.path().display().to_string();
    CliTest::new(format!(
        r#"
        pub fn run(): Number with core::system::FileSystem {{
            core::system::FileSystem::write!("{base}/a.txt", "1");
            core::system::FileSystem::write!("{base}/b.txt", "2");
            core::collections::List::length(core::system::FileSystem::list!("{base}"))
        }}
        "#
    ))
    .expect_output("2");
}

#[test]
fn test_fs_remove_then_exists_is_false() {
    let dir = TempDir::new().expect("failed to create temp dir");
    let path = dir.path().join("ephemeral.txt");
    CliTest::new(format!(
        r#"
        pub fn run(): Bool with core::system::FileSystem {{
            core::system::FileSystem::write!("{path}", "gone soon");
            core::system::FileSystem::remove!("{path}");
            core::system::FileSystem::exists!("{path}")
        }}
        "#,
        path = path.display()
    ))
    .expect_output("false");
}

#[test]
fn test_core_time_duration() {
    // Exercises core::time::Duration end to end: constructors that
    // normalize sub-second units into the seconds field, the accessors,
    // the borrow path in subtraction, and the operator/ordering traits.
    // `run` returns the number of passing checks (12 when all hold).
    CliTest::new(
        r#"
        use core::time::Duration;

        pub fn run(): Number {
            let a = Duration::from_secs(2);
            let b = Duration::from_millis(1500);   // 1s + 500_000_000ns

            let sum = a + b;                        // 3.5s
            let borrow = b - Duration::from_millis(700); // 0.8s (sub-second borrow)

            let c1 = if b.as_secs() == 1 { 1 } else { 0 };
            let c2 = if b.subsec_millis() == 500 { 1 } else { 0 };
            let c3 = if b.subsec_nanos() == 500000000 { 1 } else { 0 };
            let c4 = if sum.as_millis() == 3500 { 1 } else { 0 };
            let c5 = if sum.as_secs() == 3 { 1 } else { 0 };
            let c6 = if borrow.as_millis() == 800 { 1 } else { 0 };
            let c7 = if Duration::from_micros(1500000).as_secs() == 1 { 1 } else { 0 };
            let c8 = if Duration::from_nanos(2500000000).subsec_nanos() == 500000000 { 1 } else { 0 };
            let c9 = if a.cmp(b) == 1 { 1 } else { 0 };
            let c10 = if a == Duration::from_secs(2) { 1 } else { 0 };
            let c11 = if Duration::from_secs(0).is_zero() { 1 } else { 0 };
            let c12 = if a.as_nanos() == 2000000000 { 1 } else { 0 };

            c1 + c2 + c3 + c4 + c5 + c6 + c7 + c8 + c9 + c10 + c11 + c12
        }
        "#,
    )
    .expect_output("12");
}

#[test]
fn test_execute_run_fs_is_not_granted() {
    // FileSystem is NOT granted to executed code: only Stdio/Log are. A shipped
    // function that touches the filesystem is an unhandled-ability error,
    // not a silent escape.
    CliTest::new(
        r#"
        fn sneaky(x: Number): Number with core::system::FileSystem {
            let content = core::system::FileSystem::read!("/etc/hostname");
            x
        }

        pub fn run(): Number with core::system::Execute {
            let thunk = (x) => sneaky(x);
            let hash = core::protocol::closure_hash(thunk);
            core::system::Execute::run!(hash, 1)
        }
        "#,
    )
    .expect_error("unhandled ability");
}

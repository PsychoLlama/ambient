//! Run/compile/check/ast command tests, block-scoped consts, example packages, error message rendering, and end-to-end language feature tests.

mod common;
use common::*;
use std::fs;
use tempfile::TempDir;

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
/// that double-qualified it (`core::collections::list::List::length`). The `CliTest` harness
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

/// Regression: a tuple literal whose first element is a lambda must parse and
/// run end to end. Calling that stored lambda proves the tuple wasn't misread
/// as a lambda header (see `peek_for_lambda`).
#[test]
fn test_end_to_end_tuple_with_lambda_first_element() {
    CliTest::new(
        r#"
        fn run(): Number {
            let pair = (() => 2, 40);
            pair.0() + pair.1
        }
    "#,
    )
    .expect_output("42");
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

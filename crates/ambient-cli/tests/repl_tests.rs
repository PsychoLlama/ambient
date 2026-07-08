//! Integration tests for the REPL using the PTY-based test harness.
//!
//! These tests verify the interactive REPL experience including:
//! - Basic expression evaluation
//! - Arrow key navigation (history)
//! - Ctrl sequences
//! - Multi-step flows

mod repl_harness;

use std::path::PathBuf;

use repl_harness::{Arrow, ReplTest};

/// Path to an example project shipped in the repo.
fn example_project(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

// ─────────────────────────────────────────────────────────────────────────────
// Basic Expression Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_basic_arithmetic() {
    ReplTest::new()
        .wait_ready()
        .type_line("1 + 2")
        .expect_output("3")
        .shutdown();
}

#[test]
fn test_multiplication() {
    ReplTest::new()
        .wait_ready()
        .type_line("6 * 7")
        .expect_output("42")
        .shutdown();
}

#[test]
fn test_boolean_literal() {
    ReplTest::new()
        .wait_ready()
        .type_line("true")
        .expect_output("true")
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Definition Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_define_and_call_function() {
    ReplTest::new()
        .wait_ready()
        .type_line("fn double(x) { x * 2 }")
        .expect_output("Defined: double")
        .type_line("double(21)")
        .expect_output("42")
        .shutdown();
}

#[test]
fn test_define_constant() {
    ReplTest::new()
        .wait_ready()
        .type_line("const PI: Number = 3;")
        .expect_output("Defined: PI")
        .type_line("PI + 1")
        .expect_output("4")
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Tab Completion Tests
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// History Navigation Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_history_up_arrow() {
    ReplTest::new()
        .wait_ready()
        .type_line("1 + 1")
        .expect_output("2")
        .clear_output()
        .type_line("2 + 2")
        .expect_output("4")
        .clear_output()
        .arrow(Arrow::Up)
        .arrow(Arrow::Up)
        .enter()
        .expect_output("2") // Re-ran "1 + 1"
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// REPL Command Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_help_command() {
    ReplTest::new()
        .wait_ready()
        .type_line(":help")
        .expect_output("REPL Commands:")
        .expect_output(":quit")
        .shutdown();
}

#[test]
fn test_clear_command() {
    ReplTest::new()
        .wait_ready()
        .type_line("const X: Number = 42;")
        .expect_output("Defined: X")
        .type_line(":clear")
        .expect_output("State cleared")
        .type_line("X")
        .expect_error("undefined") // X should be gone after clear
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Ctrl Sequence Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_ctrl_c_interrupt() {
    ReplTest::new()
        .wait_ready()
        .type_text("incomplete")
        .ctrl('C')
        .expect_output("^C")
        .expect_prompt()
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Error Handling Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_parse_error() {
    ReplTest::new()
        .wait_ready()
        .type_line("fn ()")
        .expect_error("error")
        .expect_prompt() // Should still be usable
        .shutdown();
}

#[test]
fn test_undefined_variable() {
    ReplTest::new()
        .wait_ready()
        .type_line("undefined_var")
        .expect_error("undefined")
        .expect_prompt()
        .shutdown();
}

#[test]
fn test_unterminated_string_does_not_crash() {
    // Bug: unterminated strings caused a panic instead of returning an error
    ReplTest::new()
        .wait_ready()
        .type_line("\"s")
        .expect_error("unterminated") // Should show error, not crash
        .expect_prompt() // Should still be usable
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Function Inspection Bug Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_core_list_first_inspects_as_function() {
    // Bug: Submitting `core::collections::list::first` should inspect it as a function,
    // the same as if I printed the value of `fn example() {}<cr>example<cr>`.
    // Currently it might error or return something unexpected.
    ReplTest::new()
        .wait_ready()
        .type_line("core::collections::list::first")
        // Should display as a function (like "fn first<T>(list: List<T>): Option<T>")
        // or at least not error
        .expect_output("fn") // Functions should display with "fn" prefix
        .shutdown();
}

#[test]
fn test_dotted_module_path_is_rejected() {
    // Namespaces are addressed with `::`. A dotted path like `core.Number.sign`
    // is value/field access, not a namespace, so it must NOT resolve as a module
    // member — it parses as field access on the (undefined) value `core`.
    ReplTest::new()
        .wait_ready()
        .type_line("core.Number.sign")
        .expect_error("undefined")
        .shutdown();
}

#[test]
fn test_user_defined_function_inspection() {
    // For comparison: user-defined functions should also be inspectable
    ReplTest::new()
        .wait_ready()
        .type_line("fn example() { 42 }")
        .expect_output("Defined: example")
        .type_line("example")
        // Referencing a function by name should display it as a function
        .expect_output("fn") // Should show function representation
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Full-language Tests (unlocked by running the shared pipeline)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_define_struct_and_access_field() {
    // struct definitions are now supported; a value can be constructed and
    // its fields read back.
    ReplTest::new()
        .wait_ready()
        .type_line("struct Point { x: Number, y: Number }")
        .expect_output("Defined: Point")
        .type_line("Point { x: 3, y: 4 }.x")
        .expect_output("3")
        .shutdown();
}

#[test]
fn test_define_enum() {
    ReplTest::new()
        .wait_ready()
        .type_line("unique(A1B2C3D4-0000-0000-0000-000000000001) enum Color { Red, Green, Blue }")
        .expect_output("Defined: Color")
        .shutdown();
}

#[test]
fn test_define_type_alias() {
    ReplTest::new()
        .wait_ready()
        .type_line("type Count = Number;")
        .expect_output("Defined: Count")
        .shutdown();
}

#[test]
fn test_cross_turn_use_and_ability_call() {
    // A `use` committed on one turn stays in scope for later turns, and the
    // full platform ability set is wired, so a bare `Stdio::out!` runs.
    ReplTest::new()
        .wait_ready()
        .type_line("use core::system::Stdio;")
        .type_line("Stdio::out!(\"hello-repl\")")
        .expect_output("hello-repl")
        .shutdown();
}

#[test]
fn test_type_error_is_reported() {
    // A return-type mismatch is caught by the real type checker.
    ReplTest::new()
        .wait_ready()
        .type_line("fn bad(): String { 42 }")
        .expect_error("String")
        .expect_prompt()
        .shutdown();
}

#[test]
fn test_redefine_function_across_turns() {
    // Redefinition replaces the earlier same-named definition (last wins).
    ReplTest::new()
        .wait_ready()
        .type_line("fn f() { 1 }")
        .expect_output("Defined: f")
        .type_line("f()")
        .expect_output("1")
        .clear_output()
        .type_line("fn f() { 2 }")
        .expect_output("Defined: f")
        .type_line("f()")
        .expect_output("2")
        .shutdown();
}

#[test]
fn test_project_module_is_usable() {
    // Started inside a project, the REPL builds the whole package as its base
    // and project modules are reachable (fully-qualified or via `use`).
    ReplTest::with_project(&example_project("multi_module"))
        .wait_ready()
        .type_line("pkg::math_utils::gcd(48, 60)")
        .expect_output("12")
        .shutdown();
}

#[test]
fn test_non_literal_const_is_rejected() {
    // Language parity: a `const` must be a literal. A computed value (here a
    // function call) is rejected — use a `fn` instead.
    ReplTest::new()
        .wait_ready()
        .type_line("fn two() { 2 }")
        .expect_output("Defined: two")
        .type_line("const C: Number = two()")
        .expect_error("error")
        .expect_prompt()
        .shutdown();
}

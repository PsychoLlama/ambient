//! Integration tests for the REPL using the PTY-based test harness.
//!
//! These tests verify the interactive REPL experience including:
//! - Basic expression evaluation
//! - Arrow key navigation (history)
//! - Ctrl sequences
//! - Multi-step flows

mod repl_harness;

use repl_harness::{Arrow, ReplTest};

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
    // Bug: Submitting `core::List::first` should inspect it as a function,
    // the same as if I printed the value of `fn example() {}<cr>example<cr>`.
    // Currently it might error or return something unexpected.
    ReplTest::new()
        .wait_ready()
        .type_line("core::List::first")
        // Should display as a function (like "fn first<T>(list: List<T>): Option<T>")
        // or at least not error
        .expect_output("fn") // Functions should display with "fn" prefix
        .shutdown();
}

#[test]
fn test_dotted_module_path_is_rejected() {
    // Namespaces are addressed with `::`. A dotted path like `core.math.sign`
    // is value/field access, not a namespace, so it must NOT resolve as a module
    // member — it parses as field access on the (undefined) value `core`.
    ReplTest::new()
        .wait_ready()
        .type_line("core.math.sign")
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

//! Integration tests for the REPL using the PTY-based test harness.
//!
//! These tests verify the interactive REPL experience including:
//! - Basic expression evaluation
//! - Tab completion
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
        .type_line("const PI: number = 3;")
        .expect_output("Defined: PI")
        .type_line("PI + 1")
        .expect_output("4")
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Tab Completion Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_tab_completion_console() {
    ReplTest::new()
        .wait_ready()
        .type_text("Con")
        .tab()
        .expect_line("Console")
        .shutdown();
}

#[test]
fn test_tab_completion_keyword() {
    ReplTest::new()
        .wait_ready()
        .type_text("le")
        .tab()
        .expect_line("let")
        .shutdown();
}

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
        .type_line("const X: number = 42;")
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
// Completion Bug Regression Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_console_dot_completion_preserves_prefix() {
    // Bug: Console.<tab> was replacing the entire line instead of completing after the dot
    ReplTest::new()
        .wait_ready()
        .type_text("Console.")
        .tab()
        .current_line()
        .starts_with("Console.") // Should still have the Console. prefix
        .not_contains("${") // Should not have snippet placeholders
        .done()
        .shutdown();
}

#[test]
fn test_completion_no_snippet_syntax() {
    // Bug: Completions were including LSP snippet syntax like ${1:message}
    let test = ReplTest::new().wait_ready().type_text("Console.").tab();

    // Small delay to let completion happen
    std::thread::sleep(std::time::Duration::from_millis(100));

    let output = test.output();
    eprintln!("RAW OUTPUT:\n{}", output);

    // The raw output should not contain snippet syntax
    assert!(
        !output.contains("${"),
        "Output should not contain snippet syntax, but got:\n{}",
        output
    );

    test.shutdown();
}

#[test]
fn test_core_string_methods_completion() {
    // Bug: core.string. wasn't showing methods
    ReplTest::new()
        .wait_ready()
        .type_text("core.string.")
        .tab()
        .expect_line("core.string.") // Should preserve prefix and show methods
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
// Shadow Suggestion / Hint Bug Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_core_list_shadow_suggestion_shows_only_suffix() {
    // Bug: Typing `core.list` shows `core.listlist` where the second "list"
    // is shadow suggestion text. It should only show the missing segment (empty
    // in this case since `core.list` is complete), or show module members.
    let test = ReplTest::new().wait_ready().type_text("core.list");

    // Wait for hint to appear
    std::thread::sleep(std::time::Duration::from_millis(200));

    let output = test.output();
    eprintln!("RAW OUTPUT for core.list hint:\n{}", output);

    // The hint should NOT append "list" again to form "core.listlist"
    // Count occurrences of "list" after "core."
    // In the output, we should see "core.list" exactly once on the current line,
    // not "core.listlist"
    let lines: Vec<&str> = output.lines().collect();
    if let Some(prompt_line) = lines.iter().rfind(|l| l.contains("> ")) {
        assert!(
            !prompt_line.contains("core.listlist"),
            "Shadow suggestion should not duplicate 'list'. Line was: {}",
            prompt_line
        );
    }

    test.shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Core Module Member Completion Bug Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_core_list_dot_shows_function_completions() {
    // Bug: No completions for `core.list.` when it should print the functions
    // like `first`, `last`, `map`, `filter`, `fold`, etc.
    let test = ReplTest::new().wait_ready().type_text("core.list.").tab();

    // Wait for completion to process
    std::thread::sleep(std::time::Duration::from_millis(200));

    let output = test.output();
    eprintln!("RAW OUTPUT for core.list. completion:\n{}", output);

    // Should show at least one core.list function — either from the
    // compiled core module (map, filter, fold, any, all, range, sum) or
    // from the intrinsics (first, last, length, ...).
    let has_completion = output.contains("first")
        || output.contains("last")
        || output.contains("map")
        || output.contains("filter")
        || output.contains("fold")
        || output.contains("any")
        || output.contains("all")
        || output.contains("sum")
        || output.contains("length");

    assert!(
        has_completion,
        "Pressing tab after 'core.list.' should show function completions (first, last, map, etc.), but got:\n{}",
        output
    );

    test.shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Function Inspection Bug Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_core_list_first_inspects_as_function() {
    // Bug: Submitting `core::list::first` should inspect it as a function,
    // the same as if I printed the value of `fn example() {}<cr>example<cr>`.
    // Currently it might error or return something unexpected.
    ReplTest::new()
        .wait_ready()
        .type_line("core::list::first")
        // Should display as a function (like "fn first<T>(list: List<T>): Option<T>")
        // or at least not error
        .expect_output("fn") // Functions should display with "fn" prefix
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

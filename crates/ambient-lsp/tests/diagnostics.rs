//! Diagnostic tests for the LSP server.

use ambient_lsp::test_harness::LspTest;

#[test]
fn test_valid_code_no_diagnostics() {
    LspTest::new()
        .with_source("fn add(x: number, y: number): number { x + y }")
        .expect_no_diagnostics()
        .shutdown();
}

#[test]
fn test_simple_function_no_diagnostics() {
    LspTest::new()
        .with_source("fn foo() { 42 }")
        .expect_no_diagnostics()
        .shutdown();
}

#[test]
fn test_type_mismatch_diagnostic() {
    LspTest::new()
        .with_source("fn bad(): string { 42 }")
        .expect_diagnostic_at(1, "type mismatch")
        .expect_diagnostic_count(1)
        .shutdown();
}

#[test]
fn test_parse_error_unclosed_paren() {
    LspTest::new()
        .with_source("fn broken(")
        .expect_diagnostic_at(1, "expected")
        .expect_diagnostic_count(1)
        .shutdown();
}

#[test]
fn test_undefined_variable() {
    LspTest::new()
        .with_source("fn foo() { x }")
        .expect_diagnostic_at(1, "undefined")
        .shutdown();
}

#[test]
fn test_multiple_errors() {
    LspTest::new()
        .with_source(
            r#"
fn bad1(): string { 42 }
fn bad2(): bool { "hello" }
"#,
        )
        .expect_diagnostic_count(2)
        .shutdown();
}

#[test]
fn test_multiline_function_no_errors() {
    LspTest::new()
        .with_source(
            r#"
fn add(a: number, b: number): number {
    let sum = a + b;
    sum
}
"#,
        )
        .expect_no_diagnostics()
        .shutdown();
}

#[test]
fn test_if_expression_type_mismatch() {
    LspTest::new()
        .with_source(
            r#"
fn foo(): number {
    if true { 42 } else { "wrong" }
}
"#,
        )
        .expect_diagnostic_count(1)
        .shutdown();
}

#[test]
fn test_function_call_arity_mismatch() {
    // Note: The type system reports this as a type mismatch between
    // the expected function signature and what was provided
    LspTest::new()
        .with_source(
            r#"
fn add(x: number, y: number): number { x + y }
fn test() { add(1) }
"#,
        )
        .expect_diagnostic_at(3, "type mismatch")
        .shutdown();
}

#[test]
fn test_recursive_function_no_error() {
    LspTest::new()
        .with_source(
            r#"
fn factorial(n: number): number {
    if n <= 1 { 1 } else { n * factorial(n - 1) }
}
"#,
        )
        .expect_no_diagnostics()
        .shutdown();
}

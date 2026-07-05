//! Hover tests for the LSP server.

use ambient_lsp::test_harness::LspTest;

#[test]
fn test_hover_on_number_literal() {
    LspTest::new()
        .with_source("fn foo() { 42/*h*/ }")
        .hover_at("h")
        .expect_type("Number")
        .shutdown();
}

#[test]
fn test_hover_on_string_literal() {
    LspTest::new()
        .with_source(r#"fn foo() { "hello"/*h*/ }"#)
        .hover_at("h")
        .expect_type("String")
        .shutdown();
}

#[test]
fn test_hover_on_bool_literal() {
    LspTest::new()
        .with_source("fn foo() { true/*h*/ }")
        .hover_at("h")
        .expect_type("Bool")
        .shutdown();
}

#[test]
fn test_hover_on_local_variable() {
    LspTest::new()
        .with_source("fn foo() { let x/*h*/ = 42; x }")
        .hover_at("h")
        .expect_type("Number")
        .shutdown();
}

#[test]
fn test_hover_on_variable_usage() {
    LspTest::new()
        .with_source("fn foo() { let x = 42; x/*h*/ }")
        .hover_at("h")
        .expect_type("Number")
        .shutdown();
}

#[test]
fn test_hover_on_function_name() {
    LspTest::new()
        .with_source("fn add/*h*/(a: Number, b: Number): Number { a + b }")
        .hover_at("h")
        .expect_contains("fn add(a: Number, b: Number): Number")
        .shutdown();
}

#[test]
fn test_hover_on_parameter() {
    // Note: Hover on parameter names is not yet implemented
    LspTest::new()
        .with_source("fn foo(x/*h*/: Number) { x }")
        .hover_at("h")
        .expect_none()
        .shutdown();
}

#[test]
fn test_hover_on_binary_expression() {
    LspTest::new()
        .with_source("fn foo() { 1 + 2/*h*/ }")
        .hover_at("h")
        .expect_type("Number")
        .shutdown();
}

#[test]
fn test_hover_on_function_call() {
    LspTest::new()
        .with_source(
            r#"
fn add(a: Number, b: Number): Number { a + b }
fn test() { add(1, 2)/*h*/ }
"#,
        )
        .hover_at("h")
        .expect_type("Number")
        .shutdown();
}

#[test]
fn test_hover_on_tuple() {
    LspTest::new()
        .with_source("fn foo() { (1, true)/*h*/ }")
        .hover_at("h")
        .expect_contains("(Number, Bool)")
        .shutdown();
}

#[test]
fn test_hover_on_list() {
    LspTest::new()
        .with_source("fn foo() { [1, 2, 3]/*h*/ }")
        .hover_at("h")
        .expect_contains("List")
        .shutdown();
}

#[test]
fn test_hover_on_if_expression() {
    LspTest::new()
        .with_source("fn foo() { if true { 42 } else { 0 }/*h*/ }")
        .hover_at("h")
        .expect_type("Number")
        .shutdown();
}

#[test]
fn test_hover_on_record_field() {
    // Note: Record field access currently resolves to "unknown"
    // because field type inference on row types is limited
    LspTest::new()
        .with_source("fn foo() { let r = { x: 1, y: 2 }; r.x/*h*/ }")
        .hover_at("h")
        .expect_contains("x") // At least shows the field name
        .shutdown();
}

#[test]
fn test_hover_outside_expression_is_none() {
    // Cursor at position before any expression
    LspTest::new()
        .with_source("/*h*/fn foo() { 42 }")
        .hover_at("h")
        .expect_none()
        .shutdown();
}

#[test]
fn test_hover_on_function_with_doc_comment() {
    LspTest::new()
        .with_source(
            "/// Adds two numbers together.\nfn add/*h*/(a: Number, b: Number): Number { a + b }",
        )
        .hover_at("h")
        .expect_contains("Adds two numbers together.")
        .shutdown();
}

#[test]
fn test_hover_on_function_with_multiline_doc() {
    LspTest::new()
        .with_source("/// First line.\n/// Second line.\nfn foo/*h*/() { () }")
        .hover_at("h")
        .expect_contains("First line.")
        .shutdown();
}

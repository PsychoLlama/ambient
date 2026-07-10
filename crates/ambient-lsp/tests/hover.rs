//! Hover tests for the LSP server.

use ambient_lsp::test_harness::LspTest;

#[test]
fn test_hover_on_number_literal() {
    LspTest::new()
        .with_source("fn foo() { 42/*h*/ }")
        .hover_at("h")
        .expect_type("core::primitives::number")
        .shutdown();
}

#[test]
fn test_hover_on_string_literal() {
    LspTest::new()
        .with_source(r#"fn foo() { "hello"/*h*/ }"#)
        .hover_at("h")
        .expect_type("core::primitives::string")
        .shutdown();
}

#[test]
fn test_hover_on_bool_literal() {
    LspTest::new()
        .with_source("fn foo() { true/*h*/ }")
        .hover_at("h")
        .expect_type("core::primitives::bool")
        .shutdown();
}

#[test]
fn test_hover_on_local_variable() {
    LspTest::new()
        .with_source("fn foo() { let x/*h*/ = 42; x }")
        .hover_at("h")
        .expect_type("core::primitives::number")
        .shutdown();
}

#[test]
fn test_hover_on_variable_usage() {
    LspTest::new()
        .with_source("fn foo() { let x = 42; x/*h*/ }")
        .hover_at("h")
        .expect_type("core::primitives::number")
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
        .expect_type("core::primitives::number")
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
        .expect_type("core::primitives::number")
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
        .expect_type("core::primitives::number")
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

#[test]
fn test_hover_on_struct_definition() {
    LspTest::new()
        .with_source("struct Point/*h*/ { x: Number, y: Number }")
        .hover_at("h")
        .expect_contains("struct Point { x: Number, y: Number }")
        .shutdown();
}

#[test]
fn test_hover_on_unique_struct_definition() {
    LspTest::new()
        .with_source(
            "unique(A1B2C3D4-0000-0000-0000-000000000001) struct Money/*h*/ { cents: Number }",
        )
        .hover_at("h")
        .expect_contains("struct Money { cents: Number }")
        .shutdown();
}

#[test]
fn test_hover_on_type_alias_stays_type() {
    LspTest::new()
        .with_source("type Meters/*h*/ = Number;")
        .hover_at("h")
        .expect_contains("type Meters = Number")
        .shutdown();
}

#[test]
fn test_hover_on_unconstrained_throw_shows_never() {
    // Bottom elimination hands the *use site* a fresh inference variable,
    // but the expression's recorded type stays the pre-adoption `!`; when
    // nothing constrains the variable (a bare `let` of a throw), hover must
    // show the divergence, not inference-variable noise.
    LspTest::new()
        .with_source(
            r#"fn foo(): Number with Exception { let x = Exception::throw!("boom")/*h*/; 1 }"#,
        )
        .hover_at("h")
        .expect_type("```ambient\n!\n```")
        .shutdown();
}

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
    // A callsite resolves to the declaring item and shows its full signature
    // (params + return), not the bare inferred return type.
    LspTest::new()
        .with_source(
            r#"
fn add(a: Number, b: Number): Number { a + b }
fn test() { add(1, 2)/*h*/ }
"#,
        )
        .hover_at("h")
        .expect_contains("fn add(a: Number, b: Number): Number")
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
fn test_hover_on_callsite_shows_signature_and_doc() {
    // A callsite of a documented multi-param function shows the full signature
    // with parameter names AND the doc text — not the bare inferred type.
    LspTest::new()
        .with_source(
            "/// Sums two numbers.\nfn add(a: Number, b: Number): Number { a + b }\nfn test() { add/*h*/(1, 2) }",
        )
        .hover_at("h")
        .expect_contains("fn add(a: Number, b: Number): Number")
        .hover_at("h")
        .expect_contains("Sums two numbers.")
        .shutdown();
}

#[test]
fn test_hover_on_cross_module_callsite_shows_signature_and_doc() {
    // Resolving a callsite crosses module boundaries: the item's signature and
    // doc come from the declaring module's AST via the registry.
    LspTest::new()
        .with_package()
        .with_file(
            "src/mathlib.ab",
            "/// Multiplies two numbers.\npub fn mul(a: Number, b: Number): Number { a * b }",
        )
        .with_file(
            "src/main.ab",
            "use pkg::mathlib::{mul};\nfn run() { mul/*h*/(2, 3) }",
        )
        .open_file("src/main.ab")
        .hover_at("h")
        .expect_contains("fn mul(a: Number, b: Number): Number")
        .hover_at("h")
        .expect_contains("Multiplies two numbers.")
        .shutdown();
}

#[test]
fn test_hover_on_builtin_callsite_shows_signature() {
    // A builtin (`core::…`) callsite resolves through the registry to the
    // declaration's AST, so its signature and doc render like any other item.
    LspTest::new()
        .with_source("fn run() { core::convert::to_string/*h*/(42) }")
        .hover_at("h")
        .expect_contains("extern fn to_string")
        .hover_at("h")
        .expect_contains("Render any value as a human-readable string.")
        .shutdown();
}

#[test]
fn test_hover_on_callsite_has_no_effect_row_var() {
    // A function with no `with` clause generalizes its effect row; a callsite
    // instantiates a fresh unconstrained row var, which must never surface as
    // `with E<n>!` in hover.
    LspTest::new()
        .with_source("fn pure(x: Number): Number { x }\nfn test() { pure/*h*/(1) }")
        .hover_at("h")
        .expect_not_contains("with E")
        .shutdown();
}

#[test]
fn test_hover_on_lambda_local_has_no_effect_row_var() {
    // A local bound to a lambda has a function type; its (empty) effect row is
    // an unconstrained var that must not render as `with E<n>!`.
    LspTest::new()
        .with_source("fn test() { let f = (x: Number): Number => x; f/*h*/ }")
        .hover_at("h")
        .expect_not_contains("with E")
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

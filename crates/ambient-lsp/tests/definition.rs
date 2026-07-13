//! Go-to-definition tests for the LSP server.
//!
//! Note: The LSP go-to-definition returns the span of the entire definition
//! (e.g., the whole function or let statement), not just the name. Tests
//! are written to verify that go-to-definition finds *something* at the
//! expected location, rather than matching exact cursor positions.

use ambient_lsp::test_harness::LspTest;

#[test]
fn test_goto_function_definition() {
    // When clicking on a function call, it should jump to the function definition
    let (test, locations) = LspTest::new()
        .with_source(
            r#"
fn target() { 1 }
fn caller() { target/*use*/() }
"#,
        )
        .goto_definition_at("use")
        .raw();

    // Should find at least one location
    assert!(!locations.is_empty(), "Expected to find definition");

    // The definition should be on line 1 (the target function)
    assert_eq!(
        locations[0].range.start.line, 1,
        "Expected definition on line 1"
    );

    test.shutdown();
}

#[test]
fn test_goto_recursive_call() {
    // Recursive calls should find the function definition
    let (test, locations) = LspTest::new()
        .with_source(
            r#"
fn factorial(n: Number): Number {
    if n <= 1 { 1 } else { n * factorial/*use*/(n - 1) }
}
"#,
        )
        .goto_definition_at("use")
        .raw();

    assert!(!locations.is_empty(), "Expected to find definition");
    assert_eq!(
        locations[0].range.start.line, 1,
        "Expected definition on line 1"
    );

    test.shutdown();
}

#[test]
fn test_goto_no_definition_for_literal() {
    LspTest::new()
        .with_source("fn foo() { 42/*use*/ }")
        .goto_definition_at("use")
        .expect_none()
        .shutdown();
}

// Note: Tests for local variables, parameters, and lambdas are more complex
// because the LSP needs to traverse the AST to find binding definitions.
// These tests verify the infrastructure works; actual behavior depends on
// the analysis module's find_definition implementation.

#[test]
fn test_goto_definition_infrastructure() {
    // Verify that the test harness can make go-to-definition requests
    // and handle responses correctly
    let (test, locations) = LspTest::new()
        .with_source(
            r#"
fn helper() { 1 }
fn main() { helper/*use*/() }
"#,
        )
        .goto_definition_at("use")
        .raw();

    // Helper function definition should be found
    assert!(!locations.is_empty(), "Expected to find definition");

    test.shutdown();
}

// =============================================================================
// Cross-file go-to-definition tests
// =============================================================================

#[test]
fn test_goto_cross_file_imported_function() {
    // Go to definition of an imported function should jump to the other file
    let (test, locations) = LspTest::new()
        .with_package()
        .with_file("src/utils.ab", "pub fn helper(): Number { 42 }")
        .with_file(
            "src/main.ab",
            r#"
use pkg::utils::{helper};
fn run() { helper/*use*/() }
"#,
        )
        .open_file("src/main.ab")
        .goto_definition_at("use")
        .raw();

    assert!(
        !locations.is_empty(),
        "Expected to find cross-file definition"
    );

    // Should point to utils.ab
    let found_utils = locations
        .iter()
        .any(|loc| loc.uri.as_str().contains("utils.ab"));
    assert!(
        found_utils,
        "Expected definition in utils.ab, got: {:?}",
        locations.iter().map(|l| l.uri.as_str()).collect::<Vec<_>>()
    );

    test.shutdown();
}

#[test]
fn test_goto_cross_file_specific_import() {
    // Go to definition with specific item import (use pkg::module::{item})
    let (test, locations) = LspTest::new()
        .with_package()
        .with_file(
            "src/helpers.ab",
            r#"
pub fn first(): Number { 1 }
pub fn second(): Number { 2 }
"#,
        )
        .with_file(
            "src/main.ab",
            r#"
use pkg::helpers::{first, second};
fn run() { second/*use*/() }
"#,
        )
        .open_file("src/main.ab")
        .goto_definition_at("use")
        .raw();

    assert!(
        !locations.is_empty(),
        "Expected to find definition via specific import"
    );

    let found_helpers = locations
        .iter()
        .any(|loc| loc.uri.as_str().contains("helpers.ab"));
    assert!(
        found_helpers,
        "Expected definition in helpers.ab, got: {:?}",
        locations.iter().map(|l| l.uri.as_str()).collect::<Vec<_>>()
    );

    test.shutdown();
}

#[test]
fn test_goto_cross_file_multiple_functions() {
    // Test that go-to-definition works when there are multiple functions in
    // the target file and we're jumping to a specific one
    let (test, locations) = LspTest::new()
        .with_package()
        .with_file(
            "src/funcs.ab",
            r#"
pub fn first(): Number { 1 }
pub fn second(): Number { 2 }
pub fn third(): Number { 3 }
"#,
        )
        .with_file(
            "src/main.ab",
            r#"
use pkg::funcs::{second};
fn run() { second/*use*/() }
"#,
        )
        .open_file("src/main.ab")
        .goto_definition_at("use")
        .raw();

    assert!(!locations.is_empty(), "Expected to find definition");

    // Should point to funcs.ab
    let found_funcs = locations
        .iter()
        .any(|loc| loc.uri.as_str().contains("funcs.ab"));
    assert!(
        found_funcs,
        "Expected definition in funcs.ab, got: {:?}",
        locations.iter().map(|l| l.uri.as_str()).collect::<Vec<_>>()
    );

    // The definition should be on line 2 (second function)
    assert_eq!(
        locations[0].range.start.line, 2,
        "Expected definition on line 2 (the 'second' function)"
    );

    test.shutdown();
}

#[test]
fn test_goto_cross_file_expect_file_helper() {
    // Test the expect_file() assertion helper
    LspTest::new()
        .with_package()
        .with_file("src/target.ab", "pub fn defined_here(): Number { 99 }")
        .with_file(
            "src/main.ab",
            r#"
use pkg::target::{defined_here};
fn run() { defined_here/*use*/() }
"#,
        )
        .open_file("src/main.ab")
        .goto_definition_at("use")
        .expect_file("target.ab")
        .done()
        .shutdown();
}

#[test]
fn test_goto_cross_file_qualified_enum_variant() {
    // Go-to-definition on a fully-qualified foreign enum variant
    // (`shapes::Shape::Circle`, no `use` of the variant) jumps to the
    // variant's declaration. `resolve_qualified_name` used to return nothing
    // for the `Enum::Variant` spelling (the last path segment names the enum,
    // not a module); it now delegates to `resolve_item_ref`.
    LspTest::new()
        .with_package()
        .with_file(
            "src/shapes.ab",
            "pub unique(A1B2C3D4-0000-0000-0000-0000000000D1) enum Shape { Circle(Number), Square }\n",
        )
        .with_file(
            "src/main.ab",
            r#"
use pkg::shapes;
fn run(): Number { match shapes::Shape::Circle/*use*/(3.0) { shapes::Circle(n) => n, shapes::Square => 0 } }
"#,
        )
        .open_file("src/main.ab")
        .goto_definition_at("use")
        .expect_file("shapes.ab")
        .done()
        .shutdown();
}

#[test]
fn test_goto_cross_file_into_directory_module() {
    // A directory module lives at `<dir>/main.ab`, not `<dir>.ab`. Navigation
    // must target the real file — recorded at load time — rather than a path
    // reconstructed from the module name (which would point at `collections.ab`).
    LspTest::new()
        .with_package()
        .with_file("src/collections/main.ab", "pub fn seed(): Number { 1 }")
        .with_file(
            "src/main.ab",
            r#"
use pkg::collections::{seed};
fn run() { seed/*use*/() }
"#,
        )
        .open_file("src/main.ab")
        .goto_definition_at("use")
        .expect_file("collections/main.ab")
        .done()
        .shutdown();
}

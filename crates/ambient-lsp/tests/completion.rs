//! Completion tests for the LSP server.

use ambient_lsp::test_harness::LspTest;
use lsp_types::CompletionItemKind;

#[test]
fn test_keyword_completion_let() {
    LspTest::new()
        .with_source("fn foo() { le/*|*/ }")
        .complete_at("0")
        .expect_item("let")
        .expect_item_kind("let", CompletionItemKind::KEYWORD)
        .done()
        .shutdown();
}

#[test]
fn test_keyword_completion_fn() {
    LspTest::new()
        .with_source("f/*|*/")
        .complete_at("0")
        .expect_item("fn")
        .expect_item_kind("fn", CompletionItemKind::KEYWORD)
        .done()
        .shutdown();
}

#[test]
fn test_keyword_completion_if() {
    LspTest::new()
        .with_source("fn foo() { i/*|*/ }")
        .complete_at("0")
        .expect_item("if")
        .done()
        .shutdown();
}

#[test]
fn test_type_completion_number() {
    LspTest::new()
        .with_source("fn foo(x: num/*|*/)")
        .complete_at("0")
        .expect_item("number")
        .expect_item_kind("number", CompletionItemKind::TYPE_PARAMETER)
        .done()
        .shutdown();
}

#[test]
fn test_type_completion_string() {
    LspTest::new()
        .with_source("fn foo(x: str/*|*/)")
        .complete_at("0")
        .expect_item("string")
        .done()
        .shutdown();
}

#[test]
fn test_type_completion_bool() {
    LspTest::new()
        .with_source("fn foo(x: bo/*|*/)")
        .complete_at("0")
        .expect_item("bool")
        .done()
        .shutdown();
}

#[test]
fn test_ability_completion_console() {
    LspTest::new()
        .with_source("fn foo() { Con/*|*/ }")
        .complete_at("0")
        .expect_item("Console")
        .expect_item_kind("Console", CompletionItemKind::INTERFACE)
        .done()
        .shutdown();
}

#[test]
fn test_ability_method_completion_print() {
    LspTest::new()
        .with_source("fn foo() { Console.pr/*|*/ }")
        .complete_at("0")
        .expect_items(&["print!", "println!"])
        .expect_item_kind("print!", CompletionItemKind::METHOD)
        .done()
        .shutdown();
}

#[test]
fn test_ability_method_completion_all_console() {
    LspTest::new()
        .with_source("fn foo() { Console./*|*/ }")
        .complete_at("0")
        .expect_items(&["print!", "println!", "eprint!"])
        .expect_count(3)
        .done()
        .shutdown();
}

#[test]
fn test_local_variable_completion() {
    LspTest::new()
        .with_source("fn foo() { let myVariable = 1; my/*|*/ }")
        .complete_at("0")
        .expect_item("myVariable")
        .expect_item_kind("myVariable", CompletionItemKind::VARIABLE)
        .done()
        .shutdown();
}

#[test]
fn test_parameter_completion() {
    LspTest::new()
        .with_source("fn foo(param: number) { par/*|*/ }")
        .complete_at("0")
        .expect_item("param")
        .expect_item_kind("param", CompletionItemKind::VARIABLE)
        .done()
        .shutdown();
}

#[test]
fn test_function_completion() {
    LspTest::new()
        .with_source(
            r#"
fn helper() { 1 }
fn main() { hel/*|*/ }
"#,
        )
        .complete_at("0")
        .expect_item("helper")
        .expect_item_kind("helper", CompletionItemKind::FUNCTION)
        .done()
        .shutdown();
}

#[test]
fn test_multiple_local_variables() {
    LspTest::new()
        .with_source(
            r#"
fn foo() {
    let apple = 1;
    let apricot = 2;
    let banana = 3;
    ap/*|*/
}
"#,
        )
        .complete_at("0")
        .expect_items(&["apple", "apricot"])
        .expect_no_item("banana")
        .done()
        .shutdown();
}

#[test]
fn test_no_completion_for_unknown_prefix() {
    LspTest::new()
        .with_source("fn foo() { xyz123/*|*/ }")
        .complete_at("0")
        .expect_count(0)
        .done()
        .shutdown();
}

#[test]
fn test_core_module_completion() {
    LspTest::new()
        .with_source("use core.ma/*|*/")
        .complete_at("0")
        .expect_item("math")
        .expect_item_kind("math", CompletionItemKind::MODULE)
        .done()
        .shutdown();
}

#[test]
fn test_use_prefix_completion() {
    LspTest::new()
        .with_source("use pk/*|*/")
        .complete_at("0")
        .expect_item("pkg")
        .done()
        .shutdown();
}

#[test]
fn test_random_ability_methods() {
    LspTest::new()
        .with_source("fn foo() { Random./*|*/ }")
        .complete_at("0")
        .expect_items(&["seed!", "in_range!"])
        .done()
        .shutdown();
}

#[test]
fn test_time_ability_methods() {
    LspTest::new()
        .with_source("fn foo() { Time./*|*/ }")
        .complete_at("0")
        .expect_items(&["now!", "wait!"])
        .done()
        .shutdown();
}

#[test]
fn test_unique_uuid_completion() {
    let (test, items) = LspTest::new()
        .with_source("unique(/*|*/) type UserId { value: string }")
        .complete_at("0")
        .raw();

    // Should get exactly one completion
    assert_eq!(
        items.len(),
        1,
        "Expected 1 UUID completion, got {}",
        items.len()
    );

    let item = &items[0];

    // Check it's a VALUE kind
    assert_eq!(
        item.kind,
        Some(CompletionItemKind::VALUE),
        "Expected VALUE kind for UUID completion"
    );

    // Check the label looks like a UUID (8-4-4-4-12 hex format)
    let label = &item.label;
    let parts: Vec<&str> = label.split('-').collect();
    assert_eq!(
        parts.len(),
        5,
        "UUID should have 5 parts separated by dashes: {}",
        label
    );
    assert_eq!(parts[0].len(), 8, "First UUID part should be 8 chars");
    assert_eq!(parts[1].len(), 4, "Second UUID part should be 4 chars");
    assert_eq!(parts[2].len(), 4, "Third UUID part should be 4 chars");
    assert_eq!(parts[3].len(), 4, "Fourth UUID part should be 4 chars");
    assert_eq!(parts[4].len(), 12, "Fifth UUID part should be 12 chars");

    // Check detail message
    assert_eq!(
        item.detail.as_deref(),
        Some("Generated UUID for nominal type"),
        "Expected detail message for UUID completion"
    );

    test.shutdown();
}

#[test]
fn test_unique_uuid_completion_partial() {
    // Even with partial UUID already typed, should still offer completion
    let (test, items) = LspTest::new()
        .with_source("unique(abc/*|*/) type UserId { value: string }")
        .complete_at("0")
        .raw();

    // Should still offer a UUID (the word_prefix doesn't filter UUIDs)
    assert_eq!(items.len(), 1, "Expected 1 UUID completion");

    test.shutdown();
}

#[test]
fn test_unique_uuid_not_offered_after_close() {
    // After closing paren, should not offer UUID
    LspTest::new()
        .with_source("unique(abc-123) t/*|*/")
        .complete_at("0")
        .expect_item("type") // Should offer normal keyword completion
        .done()
        .shutdown();
}

//! Error-recovery parsing: `parse_recovering` returns a partial module plus
//! all errors, so IDE tooling can analyze files mid-edit.

use ambient_engine::ast::ItemKind;
use ambient_parser::{parse, parse_recovering};

/// Names of the functions in a module, in declaration order.
fn function_names(module: &ambient_engine::ast::Module) -> Vec<String> {
    module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Function(f) => Some(f.name.to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn clean_source_matches_parse() {
    let source = "fn add(x: number, y: number): number { x + y }";
    let recovered = parse_recovering(source);
    assert!(recovered.errors.is_empty());

    let full = parse(source).expect("clean source should parse");
    assert_eq!(recovered.module.items.len(), full.items.len());
}

#[test]
fn broken_item_is_skipped_and_neighbors_survive() {
    let source = r"
fn before(): number { 1 }

fn broken(: number { 2 }

fn after(): number { 3 }
";
    let recovered = parse_recovering(source);
    assert_eq!(recovered.errors.len(), 1);
    assert_eq!(function_names(&recovered.module), ["before", "after"]);
}

#[test]
fn multiple_broken_items_each_report() {
    let source = r"
fn a(): number { 1 }
fn b(: number { 2 }
fn c(): number { 3 }
fn d(] { 4 }
fn e(): number { 5 }
";
    let recovered = parse_recovering(source);
    assert_eq!(recovered.errors.len(), 2);
    assert_eq!(function_names(&recovered.module), ["a", "c", "e"]);
    // Errors arrive in source order.
    let spans: Vec<_> = recovered.errors.iter().map(|e| e.span.start).collect();
    let mut sorted = spans.clone();
    sorted.sort_unstable();
    assert_eq!(spans, sorted);
}

#[test]
fn lowering_error_is_recovered_too() {
    // A bare enum parses but fails lowering (enums require `unique(<uuid>)`).
    let source = r"
enum Shape { Circle(number) }

fn ok(): number { 1 }
";
    let recovered = parse_recovering(source);
    assert_eq!(recovered.errors.len(), 1);
    assert_eq!(function_names(&recovered.module), ["ok"]);
}

#[test]
fn lexer_error_yields_empty_module() {
    let source = "fn ok(): string { \"unterminated }";
    let recovered = parse_recovering(source);
    assert_eq!(recovered.errors.len(), 1);
    assert!(recovered.module.items.is_empty());
}

#[test]
fn incomplete_trailing_item_keeps_earlier_items() {
    // Mid-edit: the user is typing a new function at the end of the file.
    let source = r"
fn done(): number { 42 }

fn typing(
";
    let recovered = parse_recovering(source);
    assert!(!recovered.errors.is_empty());
    assert_eq!(function_names(&recovered.module), ["done"]);
}

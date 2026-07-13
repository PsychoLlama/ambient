//! Debug-info generation: source maps, local-variable names, and the
//! span → (line, column) helper.

use std::collections::HashMap;
use std::sync::Arc;

use super::entry::{compile_function_with_hash, span_to_line_col};
use super::hash::compute_temporary_hash;
use super::*;
use crate::ast::{BinaryOp, Expr, ExprKind, FunctionDef, Param, Span};
use crate::fqn::NameKey;

#[test]
fn test_debug_info_generation() {
    // Test that source maps are generated when source is provided
    let source = r"fn add(x, y) { x + y }";
    let source_file = "test.ab";

    // Create a function with spans that match the source
    let func = FunctionDef {
        name: Arc::from("add"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "x"), Param::new(1, "y")],
        ret_ty: None,
        abilities: vec![],
        // The body expression has a span covering "x + y"
        body: Expr::new(
            ExprKind::Binary {
                op: BinaryOp::Add,
                left: Box::new(Expr::local(0)),
                right: Box::new(Expr::local(1)),
                resolved_op: None,
            },
            Span::new(15, 20), // "x + y" in the source
        ),
    };

    let mut hashes = HashMap::new();
    let hash = compute_temporary_hash(&func.name);
    hashes.insert(NameKey::Bare(Arc::clone(&func.name)), hash);
    let mut ctx = ModuleContext::new(None);

    let compiled =
        compile_function_with_hash(&func, &hashes, &mut ctx, Some(source), Some(source_file))
            .expect("compilation failed");

    // Debug info should be present
    let debug_info = compiled.debug_info.expect("debug info should be generated");

    // Check function name
    assert_eq!(debug_info.function_name.as_deref(), Some("add"));

    // Check source file
    assert_eq!(debug_info.source_file.as_deref(), Some(source_file));

    // Check that source mappings were generated
    assert!(
        !debug_info.source_map.is_empty(),
        "source map should not be empty"
    );

    // Check that line/column were computed (line 1 since it's a single line)
    let first_mapping = &debug_info.source_map[0];
    assert_eq!(first_mapping.line, 1, "should be on line 1");
    assert!(
        first_mapping.column > 0,
        "column should be positive (1-indexed)"
    );

    // Check that local variable names were recorded
    assert!(
        debug_info.local_names.contains_key(&0),
        "local 'x' should be recorded"
    );
    assert!(
        debug_info.local_names.contains_key(&1),
        "local 'y' should be recorded"
    );
}

#[test]
fn test_span_to_line_col() {
    let source = "line one\nline two\nline three";

    // Line 1, column 1
    let (line, col) = span_to_line_col(source, Span::new(0, 1));
    assert_eq!((line, col), (1, 1));

    // Line 1, column 5 ("one")
    let (line, col) = span_to_line_col(source, Span::new(5, 8));
    assert_eq!((line, col), (1, 6));

    // Line 2, column 1
    let (line, col) = span_to_line_col(source, Span::new(9, 10));
    assert_eq!((line, col), (2, 1));

    // Line 3, column 6 ("three")
    let (line, col) = span_to_line_col(source, Span::new(23, 28));
    assert_eq!((line, col), (3, 6));
}

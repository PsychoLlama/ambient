use std::collections::HashMap;
use std::sync::Arc;

use super::entry::{compile_function_with_hash, span_to_line_col};
use super::hash::compute_temporary_hash;
use super::*;
use crate::ast::{BinaryOp, Expr, ExprKind, FunctionDef, Item, ItemKind, Module, Param, Span};
use crate::bytecode::CompiledFunction;
use crate::fqn::NameKey;
use crate::value::Value;

fn test_span() -> Span {
    Span::default()
}

/// Helper to compile a single function for testing.
fn compile_test_function(func: &FunctionDef) -> Result<CompiledFunction, CompileError> {
    let mut hashes = HashMap::new();
    let hash = compute_temporary_hash(&func.name);
    hashes.insert(NameKey::Bare(Arc::clone(&func.name)), hash);
    let mut ctx = ModuleContext::new(None);
    compile_function_with_hash(func, &hashes, &mut ctx, None, None)
}

/// Minimal `Option`/`Result` enum definitions in canonical variant order.
/// A registry-backed compile receives these through `imported_enums`
/// (folded in from the prelude); the compiler no longer hardcodes them, so
/// registry-less unit tests that match on `Some`/`None`/`Ok`/`Err` must
/// supply them explicitly, exactly like any other enum.
fn prelude_enum_defs() -> Vec<crate::ast::EnumDef> {
    use crate::ast::{EnumDef, EnumVariant};
    let variant = |name: &str, has_payload: bool| EnumVariant {
        name: Arc::from(name),
        payload: has_payload.then(|| crate::types::Type::named("T", vec![])),
        span: Span::default(),
    };
    vec![
        EnumDef {
            name: Arc::from("Option"),
            name_span: Span::default(),
            is_public: true,
            type_params: vec![],
            variants: vec![variant("None", false), variant("Some", true)],
            uuid: crate::types::OPTION_UUID,
        },
        EnumDef {
            name: Arc::from("Result"),
            name_span: Span::default(),
            is_public: true,
            type_params: vec![],
            variants: vec![variant("Ok", true), variant("Err", true)],
            uuid: crate::types::RESULT_UUID,
        },
    ]
}

/// Like [`compile_test_function`], but with the prelude enums registered
/// so `Some`/`None`/`Ok`/`Err` patterns resolve.
fn compile_test_function_with_prelude_enums(
    func: &FunctionDef,
) -> Result<CompiledFunction, CompileError> {
    let mut hashes = HashMap::new();
    let hash = compute_temporary_hash(&func.name);
    hashes.insert(NameKey::Bare(Arc::clone(&func.name)), hash);
    let mut ctx = ModuleContext::new(None);
    ctx.register_imported_enums(&prelude_enum_defs());
    compile_function_with_hash(func, &hashes, &mut ctx, None, None)
}

#[test]
fn test_compile_simple_function() {
    // fn add(x, y) { x + y }
    let func = FunctionDef {
        name: Arc::from("add"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "x"), Param::new(1, "y")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::binary(BinaryOp::Add, Expr::local(0), Expr::local(1)),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");

    assert_eq!(compiled.param_count, 2);
    assert!(compiled.local_count >= 2);
}

#[test]
fn test_compile_literals() {
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![],
        ret_ty: None,
        abilities: vec![],
        body: Expr::number(42.0),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");

    // Should have the number constant in the pool.
    assert!(
        compiled
            .constants
            .iter()
            .any(|v| matches!(v, Value::Number(n) if (n - 42.0).abs() < f64::EPSILON))
    );
}

#[test]
fn test_compile_if_else() {
    // fn test(x) { if x { 1 } else { 2 } }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "x")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::if_then_else(Expr::local(0), Expr::number(1.0), Some(Expr::number(2.0))),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");

    assert_eq!(compiled.param_count, 1);
}

#[test]
fn test_compile_module_with_functions() {
    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("double"),
                    name_span: Span::default(),
                    is_public: false,
                    type_params: vec![],
                    params: vec![Param::new(0, "x")],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::binary(BinaryOp::Mul, Expr::local(0), Expr::number(2.0)),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("run"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::call(Expr::name("double"), vec![Expr::number(21.0)]),
                }),
                test_span(),
            ),
        ],
    };

    let compiled = compile_module(&module).expect("compilation failed");

    assert!(compiled.entry_point.is_some());
    assert!(compiled.get_function("double").is_some());
    assert!(compiled.get_function("run").is_some());
}

#[test]
fn test_content_addressed_hash_identical_functions() {
    // Two modules with identical functions but different names should produce
    // the same content hash for those functions.
    let module1 = Module {
        name: Arc::from("test1"),
        doc: None,
        items: vec![Item::new(
            ItemKind::Function(FunctionDef {
                name: Arc::from("add_one"),
                name_span: Span::default(),
                is_public: false,
                type_params: vec![],
                params: vec![Param::new(0, "x")],
                ret_ty: None,
                abilities: vec![],
                body: Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(1.0)),
            }),
            test_span(),
        )],
    };

    let module2 = Module {
        name: Arc::from("test2"),
        doc: None,
        items: vec![Item::new(
            ItemKind::Function(FunctionDef {
                name: Arc::from("increment"), // Different name, same implementation
                name_span: Span::default(),
                is_public: false,
                type_params: vec![],
                params: vec![Param::new(0, "x")],
                ret_ty: None,
                abilities: vec![],
                body: Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(1.0)),
            }),
            test_span(),
        )],
    };

    let compiled1 = compile_module(&module1).expect("module1 compilation failed");
    let compiled2 = compile_module(&module2).expect("module2 compilation failed");

    let func1 = compiled1
        .get_function("add_one")
        .expect("add_one not found");
    let func2 = compiled2
        .get_function("increment")
        .expect("increment not found");

    // Content-addressed: identical bytecode should produce identical hash
    assert_eq!(
        func1.hash, func2.hash,
        "Identical functions with different names should have the same content hash"
    );
}

#[test]
fn test_content_addressed_hash_different_functions() {
    // Functions with different implementations should have different hashes.
    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("add_one"),
                    name_span: Span::default(),
                    is_public: false,
                    type_params: vec![],
                    params: vec![Param::new(0, "x")],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(1.0)),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("add_two"),
                    name_span: Span::default(),
                    is_public: false,
                    type_params: vec![],
                    params: vec![Param::new(0, "x")],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(2.0)),
                }),
                test_span(),
            ),
        ],
    };

    let compiled = compile_module(&module).expect("compilation failed");

    let func1 = compiled.get_function("add_one").expect("add_one not found");
    let func2 = compiled.get_function("add_two").expect("add_two not found");

    // Different implementations should have different hashes
    assert_ne!(
        func1.hash, func2.hash,
        "Functions with different implementations should have different hashes"
    );
}

#[test]
fn test_recursive_function_hash() {
    // A self-recursive function should get a stable hash
    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![Item::new(
            ItemKind::Function(FunctionDef {
                name: Arc::from("factorial"),
                name_span: Span::default(),
                is_public: false,
                type_params: vec![],
                params: vec![Param::new(0, "n")],
                ret_ty: None,
                abilities: vec![],
                // if n <= 1 { 1 } else { n * factorial(n - 1) }
                body: Expr::if_then_else(
                    Expr::binary(BinaryOp::Le, Expr::local(0), Expr::number(1.0)),
                    Expr::number(1.0),
                    Some(Expr::binary(
                        BinaryOp::Mul,
                        Expr::local(0),
                        Expr::call(
                            Expr::name("factorial"),
                            vec![Expr::binary(
                                BinaryOp::Sub,
                                Expr::local(0),
                                Expr::number(1.0),
                            )],
                        ),
                    )),
                ),
            }),
            test_span(),
        )],
    };

    let compiled = compile_module(&module).expect("compilation failed");
    let func = compiled
        .get_function("factorial")
        .expect("factorial not found");

    // Verify the hash is deterministic - compile again and check
    let compiled2 = compile_module(&module).expect("compilation failed");
    let func2 = compiled2
        .get_function("factorial")
        .expect("factorial not found");

    assert_eq!(
        func.hash, func2.hash,
        "Recursive function hash should be deterministic"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Enum Pattern Matching Tests
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_compile_match_none_pattern() {
    use crate::ast::{MatchArm, Pattern};

    // fn test(x) {
    //   match x {
    //     None => 0,
    //     _ => 1,
    //   }
    // }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "x")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::match_expr(
            Expr::local(0),
            vec![
                MatchArm::new(Pattern::variant("None", None), Expr::number(0.0)),
                MatchArm::new(Pattern::wildcard(), Expr::number(1.0)),
            ],
        ),
    };

    let compiled = compile_test_function_with_prelude_enums(&func).expect("compilation failed");
    assert_eq!(compiled.param_count, 1);
}

#[test]
fn test_compile_match_some_pattern() {
    use crate::ast::{MatchArm, Pattern};

    // fn test(x) {
    //   match x {
    //     Some(v) => v,
    //     None => 0,
    //   }
    // }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "x")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::match_expr(
            Expr::local(0),
            vec![
                MatchArm::new(
                    Pattern::variant("Some", Some(Pattern::binding(1, "v"))),
                    Expr::local(1),
                ),
                MatchArm::new(Pattern::variant("None", None), Expr::number(0.0)),
            ],
        ),
    };

    let compiled = compile_test_function_with_prelude_enums(&func).expect("compilation failed");
    assert_eq!(compiled.param_count, 1);
    // Should have at least 2 locals (param x and binding v)
    assert!(compiled.local_count >= 2);
}

#[test]
fn test_compile_match_result_patterns() {
    use crate::ast::{MatchArm, Pattern};

    // fn test(x) {
    //   match x {
    //     Ok(v) => v,
    //     Err(e) => e,
    //   }
    // }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "x")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::match_expr(
            Expr::local(0),
            vec![
                MatchArm::new(
                    Pattern::variant("Ok", Some(Pattern::binding(1, "v"))),
                    Expr::local(1),
                ),
                MatchArm::new(
                    Pattern::variant("Err", Some(Pattern::binding(2, "e"))),
                    Expr::local(2),
                ),
            ],
        ),
    };

    let compiled = compile_test_function_with_prelude_enums(&func).expect("compilation failed");
    assert_eq!(compiled.param_count, 1);
}

#[test]
fn test_compile_match_variant_with_wildcard_inner() {
    use crate::ast::{MatchArm, Pattern};

    // fn test(x) {
    //   match x {
    //     Some(_) => true,
    //     None => false,
    //   }
    // }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "x")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::match_expr(
            Expr::local(0),
            vec![
                MatchArm::new(
                    Pattern::variant("Some", Some(Pattern::wildcard())),
                    Expr::bool(true),
                ),
                MatchArm::new(Pattern::variant("None", None), Expr::bool(false)),
            ],
        ),
    };

    let compiled = compile_test_function_with_prelude_enums(&func).expect("compilation failed");
    assert_eq!(compiled.param_count, 1);
}

#[test]
fn test_debug_info_generation() {
    // Test that source maps are generated when source is provided
    let source = r#"fn add(x, y) { x + y }"#;
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

// ─────────────────────────────────────────────────────────────────────────
// Additional Expression Compilation Tests (Ticket 5.1)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_compile_unit_literal() {
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![],
        ret_ty: None,
        abilities: vec![],
        body: Expr::unit(),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");
    // Unit is compiled to Value::Unit constant
    assert!(compiled.constants.iter().any(|v| matches!(v, Value::Unit)));
}

#[test]
fn test_compile_bool_literals() {
    // Test true
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![],
        ret_ty: None,
        abilities: vec![],
        body: Expr::bool(true),
    };
    let compiled = compile_test_function(&func).expect("compilation failed");
    assert!(
        compiled
            .constants
            .iter()
            .any(|v| matches!(v, Value::Bool(true)))
    );

    // Test false
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![],
        ret_ty: None,
        abilities: vec![],
        body: Expr::bool(false),
    };
    let compiled = compile_test_function(&func).expect("compilation failed");
    assert!(
        compiled
            .constants
            .iter()
            .any(|v| matches!(v, Value::Bool(false)))
    );
}

#[test]
fn test_compile_string_literal() {
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![],
        ret_ty: None,
        abilities: vec![],
        body: Expr::string("hello world"),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");
    assert!(
        compiled
            .constants
            .iter()
            .any(|v| matches!(v, Value::String(s) if s.as_ref() == "hello world"))
    );
}

#[test]
fn test_compile_tuple() {
    // fn test() { (1, "hello", true) }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![],
        ret_ty: None,
        abilities: vec![],
        body: Expr::tuple(vec![
            Expr::number(1.0),
            Expr::string("hello"),
            Expr::bool(true),
        ]),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");
    // Should have all three constants
    assert!(
        compiled
            .constants
            .iter()
            .any(|v| matches!(v, Value::Number(n) if (n - 1.0).abs() < f64::EPSILON))
    );
    assert!(
        compiled
            .constants
            .iter()
            .any(|v| matches!(v, Value::String(s) if s.as_ref() == "hello"))
    );
    assert!(
        compiled
            .constants
            .iter()
            .any(|v| matches!(v, Value::Bool(true)))
    );
}

#[test]
fn test_compile_tuple_index() {
    // fn test(t) { t.0 }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "t")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::tuple_index(Expr::local(0), 0),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");
    assert_eq!(compiled.param_count, 1);
}

#[test]
fn test_compile_record() {
    // fn test() { { x: 1, y: 2 } }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![],
        ret_ty: None,
        abilities: vec![],
        body: Expr::record([("x", Expr::number(1.0)), ("y", Expr::number(2.0))]),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");
    // Should have both number constants
    let number_count = compiled
        .constants
        .iter()
        .filter(|v| matches!(v, Value::Number(_)))
        .count();
    assert!(number_count >= 2);
}

#[test]
fn test_compile_record_field() {
    // fn test(r) { r.x }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "r")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::field_access(Expr::local(0), "x"),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");
    assert_eq!(compiled.param_count, 1);
}

#[test]
fn test_compile_list() {
    // fn test() { [1, 2, 3] }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![],
        ret_ty: None,
        abilities: vec![],
        body: Expr::new(
            ExprKind::List(vec![
                Expr::number(1.0),
                Expr::number(2.0),
                Expr::number(3.0),
            ]),
            Span::default(),
        ),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");
    // Should have all three number constants
    let number_count = compiled
        .constants
        .iter()
        .filter(|v| matches!(v, Value::Number(_)))
        .count();
    assert!(number_count >= 3);
}

#[test]
fn test_compile_unary_neg() {
    // fn test(x) { -x }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "x")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::unary(crate::ast::UnaryOp::Neg, Expr::local(0)),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");
    assert_eq!(compiled.param_count, 1);
}

#[test]
fn test_compile_unary_not() {
    // fn test(x) { !x }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "x")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::unary(crate::ast::UnaryOp::Not, Expr::local(0)),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");
    assert_eq!(compiled.param_count, 1);
}

#[test]
fn test_compile_binary_comparison() {
    // fn test(x, y) { x == y }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "x"), Param::new(1, "y")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::binary(BinaryOp::Eq, Expr::local(0), Expr::local(1)),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");
    assert_eq!(compiled.param_count, 2);
}

#[test]
fn test_compile_binary_logical() {
    // fn test(a, b) { a && b }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "a"), Param::new(1, "b")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::binary(BinaryOp::And, Expr::local(0), Expr::local(1)),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");
    assert_eq!(compiled.param_count, 2);
}

#[test]
fn test_compile_block() {
    use crate::ast::{LetBinding, Stmt, StmtKind};

    // fn test() { let x = 1; let y = 2; x + y }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![],
        ret_ty: None,
        abilities: vec![],
        body: Expr::block(
            vec![
                Stmt::new(
                    StmtKind::Let(LetBinding {
                        id: 0,
                        name: Arc::from("x"),
                        name_span: Span::default(),
                        ty: None,
                        init: Expr::number(1.0),
                    }),
                    Span::default(),
                ),
                Stmt::new(
                    StmtKind::Let(LetBinding {
                        id: 1,
                        name: Arc::from("y"),
                        name_span: Span::default(),
                        ty: None,
                        init: Expr::number(2.0),
                    }),
                    Span::default(),
                ),
            ],
            Some(Expr::binary(BinaryOp::Add, Expr::local(0), Expr::local(1))),
        ),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");
    assert!(compiled.local_count >= 2);
}

#[test]
fn test_compile_lambda() {
    // fn test() { (x) => x + 1 }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![],
        ret_ty: None,
        abilities: vec![],
        body: Expr::lambda(
            vec![Param::new(0, "x")],
            Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(1.0)),
        ),
    };

    // Lambda compilation should succeed
    let _compiled = compile_test_function(&func).expect("compilation failed");
}

#[test]
fn test_compile_closure_capture() {
    use crate::ast::{LetBinding, Stmt, StmtKind};

    // fn test() { let y = 10; (x) => x + y }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![],
        ret_ty: None,
        abilities: vec![],
        body: Expr::block(
            vec![Stmt::new(
                StmtKind::Let(LetBinding {
                    id: 0,
                    name: Arc::from("y"),
                    name_span: Span::default(),
                    ty: None,
                    init: Expr::number(10.0),
                }),
                Span::default(),
            )],
            Some(Expr::lambda(
                vec![Param::new(1, "x")],
                Expr::binary(BinaryOp::Add, Expr::local(1), Expr::local(0)),
            )),
        ),
    };

    // Closure capturing outer variable should compile
    let _compiled = compile_test_function(&func).expect("compilation failed");
}

#[test]
fn test_compile_if_without_else() {
    // fn test(x) { if x { 1 } } - returns unit when else omitted
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "x")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::if_then_else(Expr::local(0), Expr::number(1.0), None),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");
    assert_eq!(compiled.param_count, 1);
}

#[test]
fn test_compile_nested_if() {
    // fn test(a, b) { if a { if b { 1 } else { 2 } } else { 3 } }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "a"), Param::new(1, "b")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::if_then_else(
            Expr::local(0),
            Expr::if_then_else(Expr::local(1), Expr::number(1.0), Some(Expr::number(2.0))),
            Some(Expr::number(3.0)),
        ),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");
    assert_eq!(compiled.param_count, 2);
}

#[test]
fn test_compile_all_arithmetic_ops() {
    // Test all arithmetic binary operators
    for op in [
        BinaryOp::Add,
        BinaryOp::Sub,
        BinaryOp::Mul,
        BinaryOp::Div,
        BinaryOp::Mod,
    ] {
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "x"), Param::new(1, "y")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::binary(op, Expr::local(0), Expr::local(1)),
        };

        let compiled = compile_test_function(&func).expect(&format!("{op:?} compilation failed"));
        assert_eq!(compiled.param_count, 2);
    }
}

#[test]
fn test_compile_all_comparison_ops() {
    // Test all comparison binary operators
    for op in [
        BinaryOp::Eq,
        BinaryOp::Ne,
        BinaryOp::Lt,
        BinaryOp::Le,
        BinaryOp::Gt,
        BinaryOp::Ge,
    ] {
        let func = FunctionDef {
            name: Arc::from("test"),
            name_span: Span::default(),
            is_public: false,
            type_params: vec![],
            params: vec![Param::new(0, "x"), Param::new(1, "y")],
            ret_ty: None,
            abilities: vec![],
            body: Expr::binary(op, Expr::local(0), Expr::local(1)),
        };

        let compiled = compile_test_function(&func).expect(&format!("{op:?} compilation failed"));
        assert_eq!(compiled.param_count, 2);
    }
}

#[test]
fn test_compile_match_literal_pattern() {
    use crate::ast::{Literal, MatchArm, Pattern};

    // fn test(x) { match x { 42 => true, _ => false } }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "x")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::match_expr(
            Expr::local(0),
            vec![
                MatchArm::new(Pattern::literal(Literal::Number(42.0)), Expr::bool(true)),
                MatchArm::new(Pattern::wildcard(), Expr::bool(false)),
            ],
        ),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");
    assert_eq!(compiled.param_count, 1);
}

#[test]
fn test_compile_match_binding_pattern() {
    use crate::ast::{MatchArm, Pattern};

    // fn test(x) { match x { y => y + 1 } }
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "x")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::match_expr(
            Expr::local(0),
            vec![MatchArm::new(
                Pattern::binding(1, "y"),
                Expr::binary(BinaryOp::Add, Expr::local(1), Expr::number(1.0)),
            )],
        ),
    };

    let compiled = compile_test_function(&func).expect("compilation failed");
    assert_eq!(compiled.param_count, 1);
    assert!(compiled.local_count >= 2); // x and y
}

#[test]
fn test_compile_match_tuple_pattern_unsupported() {
    use crate::ast::{MatchArm, Pattern, PatternKind};

    // Tuple patterns are not yet supported, so this should return an error
    // fn test(t) { match t { (a, b) => a + b } }
    let tuple_pat = Pattern::new(
        PatternKind::Tuple(vec![Pattern::binding(1, "a"), Pattern::binding(2, "b")]),
        Span::default(),
    );
    let func = FunctionDef {
        name: Arc::from("test"),
        name_span: Span::default(),
        is_public: false,
        type_params: vec![],
        params: vec![Param::new(0, "t")],
        ret_ty: None,
        abilities: vec![],
        body: Expr::match_expr(
            Expr::local(0),
            vec![MatchArm::new(
                tuple_pat,
                Expr::binary(BinaryOp::Add, Expr::local(1), Expr::local(2)),
            )],
        ),
    };

    // Tuple patterns are not yet supported - expect an error
    let result = compile_test_function(&func);
    assert!(result.is_err(), "Tuple patterns should be unsupported");
}

#[test]
fn test_compile_nested_function_calls() {
    // fn test() { add(mul(2, 3), mul(4, 5)) }
    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("mul"),
                    name_span: Span::default(),
                    is_public: false,
                    type_params: vec![],
                    params: vec![Param::new(0, "x"), Param::new(1, "y")],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::binary(BinaryOp::Mul, Expr::local(0), Expr::local(1)),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("add"),
                    name_span: Span::default(),
                    is_public: false,
                    type_params: vec![],
                    params: vec![Param::new(0, "x"), Param::new(1, "y")],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::binary(BinaryOp::Add, Expr::local(0), Expr::local(1)),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("run"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::call(
                        Expr::name("add"),
                        vec![
                            Expr::call(
                                Expr::name("mul"),
                                vec![Expr::number(2.0), Expr::number(3.0)],
                            ),
                            Expr::call(
                                Expr::name("mul"),
                                vec![Expr::number(4.0), Expr::number(5.0)],
                            ),
                        ],
                    ),
                }),
                test_span(),
            ),
        ],
    };

    let compiled = compile_module(&module).expect("compilation failed");
    assert!(compiled.get_function("run").is_some());
    assert!(compiled.get_function("add").is_some());
    assert!(compiled.get_function("mul").is_some());
}

#[test]
fn test_compile_mutual_recursion() {
    // fn is_even(n) { if n == 0 { true } else { is_odd(n - 1) } }
    // fn is_odd(n) { if n == 0 { false } else { is_even(n - 1) } }
    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("is_even"),
                    name_span: Span::default(),
                    is_public: false,
                    type_params: vec![],
                    params: vec![Param::new(0, "n")],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::if_then_else(
                        Expr::binary(BinaryOp::Eq, Expr::local(0), Expr::number(0.0)),
                        Expr::bool(true),
                        Some(Expr::call(
                            Expr::name("is_odd"),
                            vec![Expr::binary(
                                BinaryOp::Sub,
                                Expr::local(0),
                                Expr::number(1.0),
                            )],
                        )),
                    ),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("is_odd"),
                    name_span: Span::default(),
                    is_public: false,
                    type_params: vec![],
                    params: vec![Param::new(0, "n")],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::if_then_else(
                        Expr::binary(BinaryOp::Eq, Expr::local(0), Expr::number(0.0)),
                        Expr::bool(false),
                        Some(Expr::call(
                            Expr::name("is_even"),
                            vec![Expr::binary(
                                BinaryOp::Sub,
                                Expr::local(0),
                                Expr::number(1.0),
                            )],
                        )),
                    ),
                }),
                test_span(),
            ),
        ],
    };

    let compiled = compile_module(&module).expect("compilation failed");
    assert!(compiled.get_function("is_even").is_some());
    assert!(compiled.get_function("is_odd").is_some());
}

/// End-to-end: a module-level `const` referenced from a function body
/// compiles (its name resolves) and evaluates to the constant's value,
/// which is inlined at the reference site. The constant itself produces
/// no compiled function.
#[test]
fn module_const_compiles_and_evaluates() {
    use crate::ast::ConstDef;
    use crate::types::Type;
    use crate::value::Value;
    use crate::vm::Vm;

    // const NANOS_PER_SEC: number = 1_000_000_000
    // fn run() = NANOS_PER_SEC
    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Const(ConstDef {
                    id: 0,
                    name: Arc::from("NANOS_PER_SEC"),
                    name_span: Span::default(),
                    is_public: false,
                    ty: Some(Type::number()),
                    value: Expr::number(1_000_000_000.0),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("run"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::name("NANOS_PER_SEC"),
                }),
                test_span(),
            ),
        ],
    };

    // Type-check first (the checker registers the const so the body
    // resolves), then compile the checked module.
    let checked = crate::infer::check_module(module);
    assert!(
        checked.errors.is_empty(),
        "unexpected type errors: {:?}",
        checked.errors
    );

    let compiled = compile_module(&checked.module).expect("compilation failed");

    // The constant is a standalone value object, not a function: only
    // `run` is a compiled function.
    assert_eq!(
        compiled.functions.len(),
        1,
        "constant should not produce a compiled function"
    );
    // It does produce a content-addressed value object.
    let value_objects = compiled
        .objects
        .values()
        .filter(|o| o.as_value().is_some())
        .count();
    assert_eq!(value_objects, 1, "constant should produce one value object");

    let mut vm = Vm::new();
    for func in compiled.functions.values() {
        vm.load_function(func.clone());
    }
    for (hash, object) in &compiled.objects {
        if let Some(value) = object.as_value() {
            vm.load_value(*hash, value);
        }
    }
    let entry = compiled.entry_point.expect("entry point");
    let result = vm.call(&entry, vec![]).expect("run failed");
    assert_eq!(result, Value::Number(1_000_000_000.0));
}

/// A `const` compiles to a content-addressed value object, and a
/// referencing function links to it by hash (`LoadObject` + a dependency
/// edge) rather than inlining the literal.
#[test]
fn const_reference_links_by_hash_not_inlined() {
    use crate::ast::ConstDef;
    use crate::types::Type;
    use crate::value::Value;

    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Const(ConstDef {
                    id: 0,
                    name: Arc::from("ANSWER"),
                    name_span: Span::default(),
                    is_public: false,
                    ty: Some(Type::number()),
                    value: Expr::number(42.0),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("run"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::name("ANSWER"),
                }),
                test_span(),
            ),
        ],
    };
    let checked = crate::infer::check_module(module);
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    let compiled = compile_module(&checked.module).expect("compile");

    // Exactly one value object, whose hash is a pure function of the value.
    let expected_hash = crate::object::value_object(&Value::Number(42.0))
        .unwrap()
        .hash();
    let value_hashes: Vec<_> = compiled
        .objects
        .iter()
        .filter(|(_, o)| o.as_value().is_some())
        .map(|(h, _)| *h)
        .collect();
    assert_eq!(value_hashes, vec![expected_hash]);

    // `run` records the const hash as a dependency and emits `LoadObject`
    // — no inlined `PushConst 42` at the reference site.
    let run = compiled.get_function("run").expect("run");
    assert!(
        run.dependencies.contains(&expected_hash),
        "const hash should be a dependency"
    );
    let listing = crate::bytecode::disassemble(run);
    assert!(
        listing.contains("LoadObject"),
        "reference should compile to LoadObject: {listing}"
    );
    assert!(
        !listing.contains("PushConst"),
        "the literal must not be inlined: {listing}"
    );
}

/// A `const` written without a type annotation type-checks (the type is
/// inferred from the literal) and compiles/runs like an annotated one.
#[test]
fn const_without_annotation_infers_type() {
    use crate::ast::ConstDef;
    use crate::vm::Vm;

    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Const(ConstDef {
                    id: 0,
                    name: Arc::from("ANSWER"),
                    name_span: Span::default(),
                    is_public: false,
                    ty: None,
                    value: Expr::number(42.0),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("run"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::name("ANSWER"),
                }),
                test_span(),
            ),
        ],
    };
    let checked = crate::infer::check_module(module);
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    let compiled = compile_module(&checked.module).expect("compile");

    let mut vm = Vm::new();
    for func in compiled.functions.values() {
        vm.load_function(func.clone());
    }
    for (hash, object) in &compiled.objects {
        if let Some(value) = object.as_value() {
            vm.load_value(*hash, value);
        }
    }
    let entry = compiled.entry_point.expect("entry point");
    let result = vm.call(&entry, vec![]).expect("run failed");
    assert_eq!(result, Value::Number(42.0));
}

/// A block-scoped `const` is content-addressed exactly like a module-level
/// one: a reference links to its value object by hash (`LoadObject` + a
/// dependency edge, so the value's hash is part of the function's
/// identity), and an identical module const collapses to the same object.
#[test]
fn block_const_links_by_hash_and_dedups_with_module_const() {
    use crate::ast::{ConstDef, ExprKind, Stmt, StmtKind};
    use crate::value::Value;
    use crate::vm::Vm;

    // A module const and a block const, both `7` — content addressing
    // must collapse them to one value object.
    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Const(ConstDef {
                    id: 0,
                    name: Arc::from("SHARED"),
                    name_span: Span::default(),
                    is_public: false,
                    ty: None,
                    value: Expr::number(7.0),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("run"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::new(
                        ExprKind::Block(
                            vec![Stmt::new(
                                StmtKind::Const(ConstDef {
                                    id: 42,
                                    name: Arc::from("LOCAL"),
                                    name_span: Span::default(),
                                    is_public: false,
                                    ty: None,
                                    value: Expr::number(7.0),
                                }),
                                test_span(),
                            )],
                            Some(Box::new(Expr::name("LOCAL"))),
                        ),
                        test_span(),
                    ),
                }),
                test_span(),
            ),
        ],
    };
    let checked = crate::infer::check_module(module);
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    let compiled = compile_module(&checked.module).expect("compile");

    // Both consts share one value object, keyed purely by content.
    let expected_hash = crate::object::value_object(&Value::Number(7.0))
        .unwrap()
        .hash();
    let value_hashes: Vec<_> = compiled
        .objects
        .iter()
        .filter(|(_, o)| o.as_value().is_some())
        .map(|(h, _)| *h)
        .collect();
    assert_eq!(value_hashes, vec![expected_hash]);

    // `run` links to it by hash: dependency edge + `LoadObject`, never an
    // inlined literal.
    let run = compiled.get_function("run").expect("run");
    assert!(
        run.dependencies.contains(&expected_hash),
        "block const hash should be a dependency of the referencing function"
    );
    let listing = crate::bytecode::disassemble(run);
    assert!(
        listing.contains("LoadObject"),
        "block const reference should compile to LoadObject: {listing}"
    );

    // And it actually runs from a cold VM.
    let mut vm = Vm::new();
    for func in compiled.functions.values() {
        vm.load_function(func.clone());
    }
    for (hash, object) in &compiled.objects {
        if let Some(value) = object.as_value() {
            vm.load_value(*hash, value);
        }
    }
    let entry = compiled.entry_point.expect("entry point");
    assert_eq!(vm.call(&entry, vec![]).expect("run"), Value::Number(7.0));
}

/// Two `const`s with the same value collapse to a single value object:
/// content addressing deduplicates them, name notwithstanding.
#[test]
fn identical_consts_deduplicate_to_one_object() {
    use crate::ast::ConstDef;
    use crate::types::Type;

    let mk_const = |name: &str| {
        Item::new(
            ItemKind::Const(ConstDef {
                id: 0,
                name: Arc::from(name),
                name_span: Span::default(),
                is_public: false,
                ty: Some(Type::number()),
                value: Expr::number(100.0),
            }),
            test_span(),
        )
    };
    // Reference both so neither is dead-code-eliminated conceptually
    // (the compiler keeps all consts regardless, but this mirrors use).
    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            mk_const("A"),
            mk_const("B"),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("run"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::binary(BinaryOp::Add, Expr::name("A"), Expr::name("B")),
                }),
                test_span(),
            ),
        ],
    };
    let checked = crate::infer::check_module(module);
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    let compiled = compile_module(&checked.module).expect("compile");

    let value_object_count = compiled
        .objects
        .values()
        .filter(|o| o.as_value().is_some())
        .count();
    assert_eq!(
        value_object_count, 1,
        "two consts with the same value share one object"
    );
}

/// A named `const` binds its short name to its value-object hash in
/// `const_names` (a first-class named binding for the store), and never
/// leaks into `function_names` — a const is not a function.
#[test]
fn const_name_binds_to_value_object_hash() {
    use crate::ast::ConstDef;
    use crate::types::Type;
    use crate::value::Value;

    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![Item::new(
            ItemKind::Const(ConstDef {
                id: 0,
                name: Arc::from("ANSWER"),
                name_span: Span::default(),
                is_public: true,
                ty: Some(Type::number()),
                value: Expr::number(42.0),
            }),
            test_span(),
        )],
    };
    let checked = crate::infer::check_module(module);
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    let compiled = compile_module(&checked.module).expect("compile");

    let expected_hash = crate::object::value_object(&Value::Number(42.0))
        .unwrap()
        .hash();
    assert_eq!(
        compiled.const_names.get("ANSWER").copied(),
        Some(expected_hash),
        "const name should bind to its value-object hash"
    );
    assert!(
        !compiled.function_names.contains_key("ANSWER"),
        "a const must not appear in function_names"
    );
    // The bound hash addresses a `Value` object in the module.
    assert!(matches!(
        compiled.objects.get(&expected_hash),
        Some(crate::object::StoredObject::Value(_))
    ));
}

/// A pack round-trip preserves the function/const split: `from_pack`
/// routes each name back by the kind of object it binds (a `Value`
/// object ⇒ `const_names`, a function ⇒ `function_names`), even though
/// the pack itself carries one flat name list.
#[test]
fn pack_round_trip_preserves_const_names() {
    use crate::ast::ConstDef;
    use crate::types::Type;

    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Const(ConstDef {
                    id: 0,
                    name: Arc::from("ANSWER"),
                    name_span: Span::default(),
                    is_public: true,
                    ty: Some(Type::number()),
                    value: Expr::number(42.0),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Function(FunctionDef {
                    name: Arc::from("run"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    abilities: vec![],
                    body: Expr::name("ANSWER"),
                }),
                test_span(),
            ),
        ],
    };
    let checked = crate::infer::check_module(module);
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    let compiled = compile_module(&checked.module).expect("compile");

    let restored = CompiledModule::from_pack(&compiled.to_pack()).expect("from_pack");
    assert_eq!(
        restored.const_names.get("ANSWER").copied(),
        compiled.const_names.get("ANSWER").copied(),
        "const name should survive the pack round-trip in const_names"
    );
    assert!(
        restored.function_names.contains_key("run"),
        "function name should survive in function_names"
    );
    assert!(
        !restored.function_names.contains_key("ANSWER"),
        "a const must not be reconstructed as a function"
    );
}

/// A `const` initialized with a non-literal (here, a reference to another
/// name) is rejected by the type checker: constants map an identifier to a
/// single hashed primitive, so the initializer must be a literal.
#[test]
fn non_literal_const_is_rejected() {
    use crate::ast::ConstDef;
    use crate::infer::TypeErrorKind;
    use crate::types::Type;

    // const A: number = 1;
    // const B: number = A;   // not a literal
    let module = Module {
        name: Arc::from("test"),
        doc: None,
        items: vec![
            Item::new(
                ItemKind::Const(ConstDef {
                    id: 0,
                    name: Arc::from("A"),
                    name_span: Span::default(),
                    is_public: false,
                    ty: Some(Type::number()),
                    value: Expr::number(1.0),
                }),
                test_span(),
            ),
            Item::new(
                ItemKind::Const(ConstDef {
                    id: 0,
                    name: Arc::from("B"),
                    name_span: Span::default(),
                    is_public: false,
                    ty: Some(Type::number()),
                    value: Expr::name("A"),
                }),
                test_span(),
            ),
        ],
    };

    let checked = crate::infer::check_module(module);
    assert!(
        checked
            .errors
            .iter()
            .any(|e| matches!(e.kind, TypeErrorKind::ConstNotLiteral { .. })),
        "expected a ConstNotLiteral error, got: {:?}",
        checked.errors
    );
}

//! Expression compilation: literals, aggregates, field/index access, unary
//! and binary operators, and blocks.

use std::sync::Arc;

use super::test_support::compile_test_function;
use crate::ast::{BinaryOp, Expr, ExprKind, FunctionDef, Param, Span};
use crate::value::Value;

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

//! Control flow: conditionals, operator coverage, and multi-function
//! calling (nested and mutually recursive).

use std::sync::Arc;

use super::test_support::{compile_test_function, test_span};
use super::*;
use crate::ast::{BinaryOp, Expr, FunctionDef, Item, ItemKind, Module, Param, Span};

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

        let compiled = compile_test_function(&func).unwrap_or_else(|_| panic!("{op:?} failed"));
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

        let compiled = compile_test_function(&func).unwrap_or_else(|_| panic!("{op:?} failed"));
        assert_eq!(compiled.param_count, 2);
    }
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

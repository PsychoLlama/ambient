//! Function/module compilation basics and content-addressed hashing.

use std::sync::Arc;

use super::test_support::{compile_test_function, test_span};
use super::*;
use crate::ast::{BinaryOp, Expr, FunctionDef, Item, ItemKind, Module, Param, Span};
use crate::value::Value;

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

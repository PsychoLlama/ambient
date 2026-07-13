//! Lambda and closure compilation, including capture forwarding through
//! nested closures (the by-name capture channel).

use std::collections::HashMap;
use std::sync::Arc;

use super::entry::compile_function_with_hash;
use super::hash::compute_temporary_hash;
use super::test_support::compile_test_function;
use super::*;
use crate::ast::{BinaryOp, Expr, FunctionDef, LetBinding, Param, Span, Stmt, StmtKind};
use crate::bytecode::disassemble;
use crate::fqn::NameKey;

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
                Expr::binary(BinaryOp::Add, Expr::local(1), Expr::name("y")),
            )),
        ),
    };

    // Closure capturing outer variable should compile
    let _compiled = compile_test_function(&func).expect("compilation failed");
}

/// Regression: a free variable referenced from an *inner* lambda but declared
/// two enclosing scopes out must thread through every intervening closure as
/// an ordinary by-name capture. There is a single by-name capture channel
/// (`ExprKind::Name` and hidden dictionaries alike ride it); the middle lambda
/// forwards `y` even though it never uses `y` itself. This pins the capture
/// chain at the compiler level — the unit-level coverage that was dropped when
/// the redundant by-id capture channel was deleted, complementing the
/// end-to-end `bidirectional_lambda` LSP test.
#[test]
fn test_nested_closure_forwards_capture_by_name() {
    // fn test() { let y = 10; (a) => (b) => y + b }
    //
    // `y` is a local of `test`, captured by the *inner* lambda `(b) => y + b`
    // via `ExprKind::Name`. The *middle* lambda `(a) => …` does not mention
    // `y`, so the only way the inner lambda can see it is if the middle lambda
    // forwards it as its own capture.
    let inner = Expr::lambda(
        vec![Param::new(2, "b")],
        Expr::binary(BinaryOp::Add, Expr::name("y"), Expr::local(2)),
    );
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
            Some(Expr::lambda(vec![Param::new(1, "a")], inner)),
        ),
    };

    // Compile directly (instead of via `compile_test_function`) so the module
    // context — which owns the registered lambda functions — stays available
    // for inspection.
    let mut hashes = HashMap::new();
    let hash = compute_temporary_hash(&func.name);
    hashes.insert(NameKey::Bare(Arc::clone(&func.name)), hash);
    let mut ctx = ModuleContext::new(None);
    let outer = compile_function_with_hash(&func, &hashes, &mut ctx, None, None)
        .expect("compilation failed");

    // The two nested lambdas were registered with the context.
    assert_eq!(ctx.lambdas.len(), 2, "expected exactly two nested lambdas");

    // The enclosing `test` function builds the middle closure, loading `y`
    // from its own local and capturing it (`n=1`).
    let outer_asm = disassemble(&outer);
    assert_eq!(
        make_closure_count(&outer_asm),
        1,
        "`test` should build exactly the middle closure:\n{outer_asm}"
    );
    assert!(
        outer_asm.contains("LoadLocal") && outer_asm.contains("MakeClosure"),
        "`test` should load its local `y` and build a closure:\n{outer_asm}"
    );
    assert!(
        !outer_asm.contains("LoadCapture"),
        "`test` owns `y` as a local and captures nothing:\n{outer_asm}"
    );

    // Classify the two lambdas by whether they *build* a further closure: the
    // middle lambda does (it creates the inner one); the inner lambda does not.
    let disasms: Vec<String> = ctx
        .lambdas
        .iter()
        .map(|(_, _, func)| disassemble(func))
        .collect();
    let middle = disasms
        .iter()
        .find(|asm| make_closure_count(asm) == 1)
        .expect("one lambda must build the inner closure");
    let inner = disasms
        .iter()
        .find(|asm| make_closure_count(asm) == 0)
        .expect("one lambda must be the leaf");

    // The middle lambda forwards `y`: it reads it from *its* capture slot and
    // pushes it into the inner closure it builds. This is the capture chain.
    assert!(
        middle.contains("LoadCapture"),
        "middle lambda must forward `y` from a capture slot:\n{middle}"
    );

    // The inner lambda reads the forwarded `y` from a capture slot and builds
    // no further closure.
    assert!(
        inner.contains("LoadCapture"),
        "inner lambda must read the captured `y`:\n{inner}"
    );
}

/// Count `MakeClosure` instructions in a disassembly listing.
fn make_closure_count(disasm: &str) -> usize {
    disasm
        .lines()
        .filter(|line| line.contains("MakeClosure"))
        .count()
}

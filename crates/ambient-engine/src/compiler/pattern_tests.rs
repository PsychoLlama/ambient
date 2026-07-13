//! `match` compilation: enum-variant, literal, binding, and wildcard
//! patterns, plus the not-yet-supported tuple-pattern error path.

use std::collections::HashMap;
use std::sync::Arc;

use super::entry::compile_function_with_hash;
use super::hash::compute_temporary_hash;
use super::test_support::compile_test_function;
use super::*;
use crate::ast::{Expr, FunctionDef, Param, Span};
use crate::bytecode::CompiledFunction;
use crate::fqn::NameKey;

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
                Expr::binary(crate::ast::BinaryOp::Add, Expr::local(1), Expr::number(1.0)),
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
                Expr::binary(crate::ast::BinaryOp::Add, Expr::local(1), Expr::local(2)),
            )],
        ),
    };

    // Tuple patterns are not yet supported - expect an error
    let result = compile_test_function(&func);
    assert!(result.is_err(), "Tuple patterns should be unsupported");
}

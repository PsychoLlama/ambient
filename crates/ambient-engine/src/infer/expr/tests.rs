use super::*;
use crate::ast::{BinaryOp, HandlerLiteralMethod, MatchArm, Param, Pattern};
use crate::infer::Scheme;

#[test]
fn test_infer_literal() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    let mut expr = Expr::number(42.0);
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::number());

    let mut expr = Expr::string("hello");
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::string());

    let mut expr = Expr::bool(true);
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::bool());
}

#[test]
fn test_infer_binary_arithmetic() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    let mut expr = Expr::binary(BinaryOp::Add, Expr::number(1.0), Expr::number(2.0));
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::number());
}

#[test]
fn test_infer_binary_comparison() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    let mut expr = Expr::binary(BinaryOp::Lt, Expr::number(1.0), Expr::number(2.0));
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::bool());
}

#[test]
fn test_infer_if_then_else() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    let mut expr = Expr::if_then_else(Expr::bool(true), Expr::number(1.0), Some(Expr::number(2.0)));
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::number());
}

#[test]
fn test_infer_if_then_else_mismatch() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    let mut expr = Expr::if_then_else(
        Expr::bool(true),
        Expr::number(1.0),
        Some(Expr::string("hello")),
    );
    assert!(infer.infer_expr(&env, &mut expr).is_err());
}

#[test]
fn test_infer_tuple() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    let mut expr = Expr::tuple(vec![Expr::number(1.0), Expr::string("hello")]);
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::Tuple(vec![Type::number(), Type::string()]));
}

#[test]
fn test_infer_record() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    let mut expr = Expr::record([("x", Expr::number(1.0)), ("y", Expr::number(2.0))]);
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(
        ty,
        Type::record([("x", Type::number()), ("y", Type::number())])
    );
}

#[test]
fn test_infer_lambda() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // (x) => x + 1
    let mut expr = Expr::lambda(
        vec![Param::new(0, "x")],
        Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(1.0)),
    );
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    let ty = infer.apply(&ty);
    assert_eq!(ty, Type::function(vec![Type::number()], Type::number()));
}

#[test]
fn test_infer_lambda_call() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // ((x) => x + 1)(42)
    let lambda = Expr::lambda(
        vec![Param::new(0, "x")],
        Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(1.0)),
    );
    let mut expr = Expr::call(lambda, vec![Expr::number(42.0)]);
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::number());
}

#[test]
fn test_infer_let_polymorphism() {
    let mut infer = Infer::new();
    let mut env = TypeEnv::new();

    // identity: forall a. a -> a
    env.insert(
        0,
        "id".into(),
        Scheme::poly(vec![0], Type::function(vec![Type::var(0)], Type::var(0))),
    );

    // id(42) should be number
    let mut expr = Expr::call(Expr::local(0), vec![Expr::number(42.0)]);
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::number());

    // id("hello") should be string
    let mut expr = Expr::call(Expr::local(0), vec![Expr::string("hello")]);
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::string());
}

#[test]
fn test_generalize() {
    let infer = Infer::new();
    let env = TypeEnv::new();

    // A type with free variable should generalize
    let ty = Type::function(vec![Type::var(0)], Type::var(0));
    let scheme = infer.generalize(&env, &ty);

    assert_eq!(scheme.vars, vec![0]);
}

#[test]
fn test_instantiate() {
    let mut infer = Infer::new();

    let scheme = Scheme::poly(vec![0], Type::function(vec![Type::var(0)], Type::var(0)));
    let ty = infer.instantiate(&scheme);

    // Should get a fresh type variable, not '0
    if let Type::Function(f) = ty {
        assert!(matches!(f.params[0], Type::Var(_)));
        assert!(matches!(*f.ret, Type::Var(_)));
    } else {
        panic!("Expected function type");
    }
}

#[test]
fn test_resolve_holes_simple() {
    let mut infer = Infer::new();

    // Hole becomes a fresh type variable
    let resolved = infer.resolve_holes(&Type::Hole);
    assert!(matches!(resolved, Type::Var(_)));
}

#[test]
fn test_resolve_holes_nested() {
    let mut infer = Infer::new();

    // Holes in nested types get resolved
    let func = Type::function(vec![Type::Hole], Type::Hole);
    let resolved = infer.resolve_holes(&func);

    if let Type::Function(f) = resolved {
        assert!(matches!(f.params[0], Type::Var(_)));
        assert!(matches!(*f.ret, Type::Var(_)));
    } else {
        panic!("Expected function type");
    }
}

#[test]
fn test_resolve_holes_partial() {
    let mut infer = Infer::new();

    // Mix of concrete types and holes
    let tuple = Type::Tuple(vec![Type::number(), Type::Hole, Type::string()]);
    let resolved = infer.resolve_holes(&tuple);

    if let Type::Tuple(elems) = resolved {
        assert_eq!(elems[0], Type::number());
        assert!(matches!(elems[1], Type::Var(_)));
        assert_eq!(elems[2], Type::string());
    } else {
        panic!("Expected tuple type");
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Handler literal type checking tests (Milestone 13)
// ─────────────────────────────────────────────────────────────────────────

/// An `Infer` with prelude-style test abilities registered:
/// `Printer.go(message: string): ()` and
/// `Clock { now(): number; wait(duration: number): (); }`.
fn infer_with_test_prelude() -> Infer {
    use crate::ability_resolver::{DynAbility, DynMethod};

    let mut infer = Infer::new();
    infer.ability_resolver.register_dynamic_in_namespace(
        &crate::fqn::ModuleId::core_system(),
        DynAbility {
            id: crate::types::AbilityId::from_bytes([7; 32]),
            uuid: uuid::Uuid::from_u128(7),
            name: "Printer".into(),
            methods: vec![DynMethod {
                name: "go".into(),
                param_names: vec![],
                params: vec![Type::string()],
                ret: Type::Unit,
                quantified: vec![],
                type_param_names: vec![],
                quantified_abilities: vec![],
                bounds: Vec::new(),
                signature: ambient_core::SignatureHash::new(&["string"], "unit"),
                has_impl: true,
            }],
            dependencies: vec![],
        },
    );
    infer.ability_resolver.register_dynamic_in_namespace(
        &crate::fqn::ModuleId::core_system(),
        DynAbility {
            id: crate::types::AbilityId::from_bytes([8; 32]),
            uuid: uuid::Uuid::from_u128(8),
            name: "Clock".into(),
            methods: vec![
                DynMethod {
                    name: "now".into(),
                    param_names: vec![],
                    params: vec![],
                    ret: Type::number(),
                    quantified: vec![],
                    type_param_names: vec![],
                    quantified_abilities: vec![],
                    bounds: Vec::new(),
                    signature: ambient_core::SignatureHash::new(&[] as &[&str], "number"),
                    has_impl: true,
                },
                DynMethod {
                    name: "wait".into(),
                    param_names: vec![],
                    params: vec![Type::number()],
                    ret: Type::Unit,
                    quantified: vec![],
                    type_param_names: vec![],
                    quantified_abilities: vec![],
                    bounds: Vec::new(),
                    signature: ambient_core::SignatureHash::new(&["number"], "unit"),
                    has_impl: true,
                },
            ],
            dependencies: vec![],
        },
    );
    infer
}

/// A `core::system::<name>` ability reference (the namespace test
/// prelude abilities are registered under).
fn core_ability(name: &str) -> crate::ast::QualifiedName {
    crate::ast::QualifiedName::qualified(vec!["core", "system"], name)
}

/// Build a handler-literal arm `ability::method(params) => body`.
fn arm(
    ability: crate::ast::QualifiedName,
    method: &str,
    params: Vec<Param>,
    body: Expr,
) -> HandlerLiteralMethod {
    HandlerLiteralMethod {
        ability,
        method: method.into(),
        method_span: crate::ast::Span::default(),
        params,
        body,
        span: crate::ast::Span::default(),
    }
}

/// A `resume(value)` expression (its type is the handler's answer type).
fn resume(value: Expr) -> Expr {
    Expr::new(
        ExprKind::Resume(Box::new(value)),
        crate::ast::Span::default(),
    )
}

#[test]
fn test_handler_literal_prelude_ability() {
    let mut infer = infer_with_test_prelude();
    let env = TypeEnv::new();

    // { core::system::Printer::go(msg) => resume(()) }
    let mut expr = Expr::handler_literal(vec![arm(
        core_ability("Printer"),
        "go",
        vec![Param::new(1, "msg")],
        resume(Expr::unit()),
    )]);

    let ty = infer.infer_expr(&env, &mut expr).unwrap();

    // Should infer Handler<Printer, R>
    if let Type::Handler(handler_ty) = ty {
        assert_eq!(
            handler_ty.ability,
            crate::types::AbilityId::from_bytes([7; 32])
        );
    } else {
        panic!("Expected Handler type, got {ty:?}");
    }
}

#[test]
fn test_handler_literal_exception_throw() {
    use crate::ability_resolver::{DynAbility, DynMethod};

    let mut infer = Infer::new();
    // `Exception` is a module-declared ability (`core::exception`,
    // prelude-injected). A real check resolves the bare name through the
    // prelude; here — registry-less — register it as a local dynamic under
    // its content-addressed identity so the bare reference resolves.
    infer.ability_resolver.register_dynamic(DynAbility {
        id: ambient_core::exception::ability_id(),
        uuid: ambient_core::exception::EXCEPTION_UUID,
        name: "Exception".into(),
        methods: vec![DynMethod {
            name: "throw".into(),
            param_names: vec!["message".into()],
            params: vec![Type::string()],
            ret: Type::Never,
            quantified: vec![],
            type_param_names: vec![],
            quantified_abilities: vec![],
            bounds: Vec::new(),
            signature: ambient_core::exception::throw_signature(),
            has_impl: false,
        }],
        dependencies: vec![],
    });
    let env = TypeEnv::new();

    // { Exception::throw(err) => () } — a non-resuming arm pins R to unit.
    let mut expr = Expr::handler_literal(vec![arm(
        crate::ast::QualifiedName::simple("Exception"),
        "throw",
        vec![Param::new(1, "err")],
        Expr::unit(),
    )]);

    let ty = infer.infer_expr(&env, &mut expr).unwrap();

    // Should infer Handler<Exception, ()>
    if let Type::Handler(handler_ty) = ty {
        assert_eq!(handler_ty.ability, ambient_core::exception::ability_id());
        assert_eq!(*handler_ty.answer, Type::Unit);
    } else {
        panic!("Expected Handler type, got {ty:?}");
    }
}

#[test]
fn test_handler_literal_multi_method() {
    let mut infer = infer_with_test_prelude();
    let env = TypeEnv::new();

    // { Clock::now() => resume(0.0), Clock::wait(duration) => resume(()) }
    let mut expr = Expr::handler_literal(vec![
        arm(
            core_ability("Clock"),
            "now",
            vec![],
            resume(Expr::number(0.0)),
        ),
        arm(
            core_ability("Clock"),
            "wait",
            vec![Param::new(1, "duration")],
            resume(Expr::unit()),
        ),
    ]);

    let ty = infer.infer_expr(&env, &mut expr).unwrap();

    // Should infer Handler<Clock, R>
    if let Type::Handler(handler_ty) = ty {
        assert_eq!(
            handler_ty.ability,
            crate::types::AbilityId::from_bytes([8; 32])
        );
    } else {
        panic!("Expected Handler type, got {ty:?}");
    }
}

#[test]
fn test_handler_literal_multiple_abilities_rejected() {
    let mut infer = infer_with_test_prelude();
    let env = TypeEnv::new();

    // A handler *value* covering two abilities is a type error.
    let mut expr = Expr::handler_literal(vec![
        arm(
            core_ability("Printer"),
            "go",
            vec![Param::new(1, "msg")],
            resume(Expr::unit()),
        ),
        arm(
            core_ability("Clock"),
            "now",
            vec![],
            resume(Expr::number(0.0)),
        ),
    ]);

    let result = infer.infer_expr(&env, &mut expr);
    assert!(
        matches!(
            result,
            Err(ref e) if matches!(e.kind, TypeErrorKind::HandlerValueMultipleAbilities { .. })
        ),
        "a multi-ability handler value should be rejected, got {result:?}"
    );
}

#[test]
fn test_handler_literal_unknown_method() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // { Exception::unknown_method(x) => ... } - no such method.
    let mut expr = Expr::handler_literal(vec![arm(
        crate::ast::QualifiedName::simple("Exception"),
        "unknown_method",
        vec![Param::new(1, "x")],
        Expr::unit(),
    )]);

    let result = infer.infer_expr(&env, &mut expr);
    assert!(
        result.is_err(),
        "Should fail when a method is not on the ability"
    );
}

#[test]
fn test_handler_literal_wrong_arity() {
    let mut infer = infer_with_test_prelude();
    let env = TypeEnv::new();

    // { Printer::go(a, b) => ... } - Printer.go takes 1 arg, not 2
    let mut expr = Expr::handler_literal(vec![arm(
        core_ability("Printer"),
        "go",
        vec![Param::new(1, "a"), Param::new(2, "b")],
        resume(Expr::unit()),
    )]);

    let result = infer.infer_expr(&env, &mut expr);
    assert!(result.is_err(), "Should fail when arity doesn't match");

    // Check error message mentions arity
    if let Err(err) = result {
        let msg = format!("{}", err.kind);
        assert!(
            msg.contains("expects 1 parameters") || msg.contains("expected 1"),
            "Error should mention expected arity: {msg}"
        );
    }
}

#[test]
fn test_handler_literal_partial_handler() {
    let mut infer = infer_with_test_prelude();
    let env = TypeEnv::new();

    // { Clock::now() => resume(0.0) } - only handles now, not wait
    // This should be allowed (partial handlers can be composed)
    let mut expr = Expr::handler_literal(vec![arm(
        core_ability("Clock"),
        "now",
        vec![],
        resume(Expr::number(0.0)),
    )]);

    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert!(
        matches!(ty, Type::Handler(_)),
        "Partial handlers should be allowed"
    );
}

#[test]
fn test_handler_literal_params_take_declared_types() {
    let mut infer = infer_with_test_prelude();
    let env = TypeEnv::new();

    // { Printer::go(msg) => msg + "!" } — Printer.go(message: string), so
    // msg is a string and string concatenation type-checks.
    let mut expr = Expr::handler_literal(vec![arm(
        core_ability("Printer"),
        "go",
        vec![Param::new(1, "msg")],
        Expr::binary(BinaryOp::Add, Expr::local(1), Expr::string("!")),
    )]);
    let result = infer.infer_expr(&env, &mut expr);
    assert!(
        result.is_ok(),
        "handler param should have its declared type in scope: {result:?}"
    );

    // { Printer::go(msg) => msg + 1 } — msg is a string, not a number: rejected.
    let mut expr = Expr::handler_literal(vec![arm(
        core_ability("Printer"),
        "go",
        vec![Param::new(1, "msg")],
        Expr::binary(BinaryOp::Add, Expr::local(1), Expr::number(1.0)),
    )]);
    let result = infer.infer_expr(&env, &mut expr);
    assert!(
        result.is_err(),
        "handler param must be constrained to the declared param type"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Error case coverage tests (CQ-012)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_error_undefined_variable() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // Reference to a variable that doesn't exist
    let mut expr = Expr::variable("undefined_var");
    let result = infer.infer_expr(&env, &mut expr);

    assert!(result.is_err());
    if let Err(err) = result {
        assert!(
            matches!(err.kind, TypeErrorKind::UndefinedVariable { .. }),
            "Expected UndefinedVariable, got {:?}",
            err.kind
        );
    }
}

#[test]
fn test_error_field_not_found() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // Access a field that doesn't exist on a record
    let record = Expr::record([("x", Expr::number(1.0)), ("y", Expr::number(2.0))]);
    let mut expr = Expr::field_access(record, "z");
    let result = infer.infer_expr(&env, &mut expr);

    assert!(result.is_err());
    if let Err(err) = result {
        assert!(
            matches!(err.kind, TypeErrorKind::FieldNotFound { .. }),
            "Expected FieldNotFound, got {:?}",
            err.kind
        );
    }
}

#[test]
fn test_error_tuple_index_out_of_bounds() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // Access index 5 on a 2-element tuple
    let tuple = Expr::tuple(vec![Expr::number(1.0), Expr::number(2.0)]);
    let mut expr = Expr::tuple_index(tuple, 5);
    let result = infer.infer_expr(&env, &mut expr);

    assert!(result.is_err());
    if let Err(err) = result {
        assert!(
            matches!(err.kind, TypeErrorKind::TupleIndexOutOfBounds { .. }),
            "Expected TupleIndexOutOfBounds, got {:?}",
            err.kind
        );
    }
}

#[test]
fn test_error_calling_non_function() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // Try to call a number as a function - type inference will produce TypeMismatch
    // because it tries to unify Number with Function type
    let mut expr = Expr::call(Expr::number(42.0), vec![Expr::number(1.0)]);
    let result = infer.infer_expr(&env, &mut expr);

    assert!(result.is_err());
    if let Err(err) = result {
        // The error is TypeMismatch because unification fails when trying
        // to match Number with a function type
        assert!(
            matches!(err.kind, TypeErrorKind::TypeMismatch { .. }),
            "Expected TypeMismatch when calling non-function, got {:?}",
            err.kind
        );
    }
}

#[test]
fn test_error_non_boolean_if_condition() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // if with a number condition instead of bool - produces TypeMismatch
    // when unifying condition type (Number) with Bool
    let mut expr = Expr::if_then_else(
        Expr::number(1.0),
        Expr::number(2.0),
        Some(Expr::number(3.0)),
    );
    let result = infer.infer_expr(&env, &mut expr);

    assert!(result.is_err());
    if let Err(err) = result {
        // Unification error: expected Number (condition type), actual Bool (target type)
        assert!(
            matches!(err.kind, TypeErrorKind::TypeMismatch { .. }),
            "Expected TypeMismatch for non-bool condition, got {:?}",
            err.kind
        );
    }
}

#[test]
fn test_error_match_arms_different_types() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // Match with arms returning different types - produces TypeMismatch
    // when unifying first arm type with subsequent arm types
    let mut expr = Expr::match_expr(
        Expr::number(1.0),
        vec![
            MatchArm::new(Pattern::wildcard(), Expr::number(1.0)),
            MatchArm::new(Pattern::wildcard(), Expr::string("hello")),
        ],
    );
    let result = infer.infer_expr(&env, &mut expr);

    assert!(result.is_err());
    if let Err(err) = result {
        assert!(
            matches!(
                &err.kind,
                TypeErrorKind::TypeMismatch { expected, actual }
                    if expected.as_primitive() == Some(crate::types::Primitive::Number)
                        && actual.as_primitive() == Some(crate::types::Primitive::String)
            ),
            "Expected TypeMismatch between Number and String, got {:?}",
            err.kind
        );
    }
}

#[test]
fn test_error_wrong_argument_count() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // Call a function with wrong number of arguments - produces TypeMismatch
    // because function types don't match
    let lambda = Expr::lambda(
        vec![Param::new(0, "x"), Param::new(1, "y")],
        Expr::binary(BinaryOp::Add, Expr::local(0), Expr::local(1)),
    );
    let mut expr = Expr::call(lambda, vec![Expr::number(1.0)]); // Only 1 arg, needs 2
    let result = infer.infer_expr(&env, &mut expr);

    assert!(result.is_err());
    // This produces a TypeMismatch because the inferred function type
    // doesn't match the application
    if let Err(err) = result {
        assert!(
            matches!(err.kind, TypeErrorKind::TypeMismatch { .. }),
            "Expected TypeMismatch for wrong argument count, got {:?}",
            err.kind
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Never (`!`) semantics: bottom elimination and catch-only arms
// ─────────────────────────────────────────────────────────────────────────────

/// An `Infer` with the prelude `Exception` registered as a bare dynamic
/// (a real check resolves it through the prelude; these tests are
/// registry-less).
fn infer_with_exception() -> Infer {
    use crate::ability_resolver::{DynAbility, DynMethod};
    let mut infer = Infer::new();
    infer.ability_resolver.register_dynamic(DynAbility {
        id: ambient_core::exception::ability_id(),
        uuid: ambient_core::exception::EXCEPTION_UUID,
        name: "Exception".into(),
        methods: vec![DynMethod {
            name: "throw".into(),
            param_names: vec!["message".into()],
            params: vec![Type::string()],
            ret: Type::Never,
            quantified: vec![],
            type_param_names: vec![],
            quantified_abilities: vec![],
            bounds: Vec::new(),
            signature: ambient_core::exception::throw_signature(),
            has_impl: false,
        }],
        dependencies: vec![],
    });
    infer
}

/// A `Exception::throw!(msg)` perform expression.
fn throw_expr(msg: &str) -> Expr {
    Expr::new(
        ExprKind::Perform(crate::ast::AbilityCall {
            ability: Some(crate::ast::QualifiedName::simple("Exception")),
            method: "throw".into(),
            method_span: crate::ast::Span::default(),
            args: vec![Expr::string(msg)],
            fingerprints: None,
            span: crate::ast::Span::default(),
        }),
        crate::ast::Span::default(),
    )
}

#[test]
fn test_never_perform_adopts_the_other_branch_type() {
    // The motivating example: a throwing branch must unify with the
    // concrete type the other branch produces — in either order.
    let mut infer = infer_with_exception();
    let env = TypeEnv::new();

    let mut expr = Expr::if_then_else(
        Expr::bool(true),
        Expr::number(1.0),
        Some(throw_expr("too low")),
    );
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(infer.apply(&ty), Type::number());

    let mut flipped = Expr::if_then_else(
        Expr::bool(true),
        throw_expr("too low"),
        Some(Expr::number(1.0)),
    );
    let ty = infer.infer_expr(&env, &mut flipped).unwrap();
    assert_eq!(infer.apply(&ty), Type::number());
}

#[test]
fn test_never_perform_satisfies_a_declared_never() {
    // Bottom elimination must not break bottom introduction: a `!`-typed
    // producer still checks against a declared `!` (the adopted variable
    // binds to `Never`), while a real value never does.
    let mut infer = infer_with_exception();
    let env = TypeEnv::new();

    let mut diverges = throw_expr("boom");
    let ty = infer.infer_expr(&env, &mut diverges).unwrap();
    assert!(infer.unify(&Type::Never, &ty, (0, 0)).is_ok());

    let mut value = Expr::number(42.0);
    let ty = infer.infer_expr(&env, &mut value).unwrap();
    assert!(
        infer.unify(&Type::Never, &ty, (0, 0)).is_err(),
        "a concrete value must not check against a declared `!`"
    );
}

#[test]
fn test_resume_in_never_arm_is_a_dedicated_error() {
    // `throw` returns `!`: the perform site unwinds and no continuation
    // exists, so `resume` in its arm is rejected outright — even when the
    // resume argument is itself never-typed (which adoption would let
    // through value unification).
    let mut infer = infer_with_exception();
    let env = TypeEnv::new();

    let mut expr = Expr::handler_literal(vec![arm(
        crate::ast::QualifiedName::simple("Exception"),
        "throw",
        vec![Param::new(1, "err")],
        resume(throw_expr("rethrow")),
    )]);

    let err = infer.infer_expr(&env, &mut expr).unwrap_err();
    assert!(
        matches!(err.kind, TypeErrorKind::ResumeNeverMethod { .. }),
        "expected ResumeNeverMethod, got {:?}",
        err.kind
    );
}

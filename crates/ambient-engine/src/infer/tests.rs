//! Tests for type inference.

use super::*;
use crate::ast::{BinaryOp, HandlerLiteralMethod, MatchArm, Param, Pattern};

fn span() -> (u32, u32) {
    (0, 0)
}

#[test]
fn test_unify_primitives() {
    let mut infer = Infer::new();
    assert!(infer.unify(&Type::Number, &Type::Number, span()).is_ok());
    assert!(infer.unify(&Type::String, &Type::String, span()).is_ok());
    assert!(infer.unify(&Type::Bool, &Type::Bool, span()).is_ok());
    assert!(infer.unify(&Type::Unit, &Type::Unit, span()).is_ok());
}

#[test]
fn test_unify_mismatch() {
    let mut infer = Infer::new();
    assert!(infer.unify(&Type::Number, &Type::String, span()).is_err());
    assert!(infer.unify(&Type::Bool, &Type::Number, span()).is_err());
}

#[test]
fn test_unify_type_variable() {
    let mut infer = Infer::new();
    let var = infer.fresh();
    assert!(infer.unify(&var, &Type::Number, span()).is_ok());
    assert_eq!(infer.apply(&var), Type::Number);
}

#[test]
fn test_unify_tuples() {
    let mut infer = Infer::new();
    let t1 = Type::Tuple(vec![Type::Number, Type::String]);
    let t2 = Type::Tuple(vec![Type::Number, Type::String]);
    assert!(infer.unify(&t1, &t2, span()).is_ok());
}

#[test]
fn test_unify_tuples_mismatch() {
    let mut infer = Infer::new();
    let t1 = Type::Tuple(vec![Type::Number, Type::String]);
    let t2 = Type::Tuple(vec![Type::Number, Type::Bool]);
    assert!(infer.unify(&t1, &t2, span()).is_err());
}

#[test]
fn test_unify_records() {
    let mut infer = Infer::new();
    let r1 = Type::record([("x", Type::Number), ("y", Type::String)]);
    let r2 = Type::record([("x", Type::Number), ("y", Type::String)]);
    assert!(infer.unify(&r1, &r2, span()).is_ok());
}

#[test]
fn test_unify_functions() {
    let mut infer = Infer::new();
    let f1 = Type::function(vec![Type::Number], Type::String);
    let f2 = Type::function(vec![Type::Number], Type::String);
    assert!(infer.unify(&f1, &f2, span()).is_ok());
}

#[test]
fn test_occurs_check() {
    let mut infer = Infer::new();
    let var = infer.fresh();
    // Try to unify 'a with ('a -> 'a), should fail
    let fn_ty = Type::function(vec![var.clone()], var.clone());
    assert!(infer.unify(&var, &fn_ty, span()).is_err());
}

#[test]
fn test_infer_literal() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    let mut expr = Expr::number(42.0);
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::Number);

    let mut expr = Expr::string("hello");
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::String);

    let mut expr = Expr::bool(true);
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::Bool);
}

#[test]
fn test_infer_binary_arithmetic() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    let mut expr = Expr::binary(BinaryOp::Add, Expr::number(1.0), Expr::number(2.0));
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::Number);
}

#[test]
fn test_infer_binary_comparison() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    let mut expr = Expr::binary(BinaryOp::Lt, Expr::number(1.0), Expr::number(2.0));
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::Bool);
}

#[test]
fn test_infer_if_then_else() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    let mut expr = Expr::if_then_else(Expr::bool(true), Expr::number(1.0), Some(Expr::number(2.0)));
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::Number);
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
    assert_eq!(ty, Type::Tuple(vec![Type::Number, Type::String]));
}

#[test]
fn test_infer_record() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    let mut expr = Expr::record([("x", Expr::number(1.0)), ("y", Expr::number(2.0))]);
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::record([("x", Type::Number), ("y", Type::Number)]));
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
    assert_eq!(ty, Type::function(vec![Type::Number], Type::Number));
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
    assert_eq!(ty, Type::Number);
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
    assert_eq!(ty, Type::Number);

    // id("hello") should be string
    let mut expr = Expr::call(Expr::local(0), vec![Expr::string("hello")]);
    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert_eq!(ty, Type::String);
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
        assert!(matches!(f.params[0], Type::Var(TypeVar::Unbound(_))));
        assert!(matches!(*f.ret, Type::Var(TypeVar::Unbound(_))));
    } else {
        panic!("Expected function type");
    }
}

#[test]
fn test_type_error_display() {
    let err = TypeError::new(
        TypeErrorKind::TypeMismatch {
            expected: Type::Number,
            actual: Type::String,
        },
        (0, 10),
    );
    let msg = format!("{err}");
    assert!(msg.contains("type mismatch"));
    assert!(msg.contains("number"));
    assert!(msg.contains("string"));
}

// ─────────────────────────────────────────────────────────────────────────
// Ability type inference tests (Milestone 8)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_unify_empty_abilities() {
    let mut infer = Infer::new();
    let result = infer.unify_abilities(&AbilitySet::Empty, &AbilitySet::Empty, span());
    assert!(result.is_ok());
}

#[test]
fn test_unify_same_abilities() {
    let mut infer = Infer::new();
    let a = AbilitySet::from_abilities([1, 2]);
    let b = AbilitySet::from_abilities([1, 2]);
    let result = infer.unify_abilities(&a, &b, span());
    assert!(result.is_ok());
}

#[test]
fn test_unify_different_abilities_fails() {
    let mut infer = Infer::new();
    let a = AbilitySet::from_abilities([1, 2]);
    let b = AbilitySet::from_abilities([1, 3]);
    let result = infer.unify_abilities(&a, &b, span());
    assert!(result.is_err());
}

#[test]
fn test_unify_ability_var_with_concrete() {
    let mut infer = Infer::new();
    let var = AbilitySet::var(0);
    let concrete = AbilitySet::from_abilities([1, 2]);
    let result = infer.unify_abilities(&var, &concrete, span());
    assert!(result.is_ok());

    // The variable should now be bound to the concrete set
    let applied = infer.apply_abilities(&var);
    assert_eq!(applied, concrete);
}

#[test]
fn test_unify_ability_var_with_empty() {
    let mut infer = Infer::new();
    let var = AbilitySet::var(0);
    let result = infer.unify_abilities(&var, &AbilitySet::Empty, span());
    assert!(result.is_ok());

    let applied = infer.apply_abilities(&var);
    assert_eq!(applied, AbilitySet::Empty);
}

#[test]
fn test_unify_same_ability_var() {
    let mut infer = Infer::new();
    let var = AbilitySet::var(0);
    let result = infer.unify_abilities(&var, &var, span());
    assert!(result.is_ok());
}

#[test]
fn test_ability_tracking() {
    let mut infer = Infer::new();

    // Start with empty abilities
    assert!(infer.current_abilities().is_pure());

    // Require an ability
    infer.require_ability(1);
    assert!(infer.current_abilities().contains(1));

    // Require another ability
    infer.require_ability(2);
    assert!(infer.current_abilities().contains(1));
    assert!(infer.current_abilities().contains(2));

    // Reset
    infer.reset_abilities();
    assert!(infer.current_abilities().is_pure());
}

#[test]
fn test_fresh_ability_var() {
    let mut infer = Infer::new();
    let v1 = infer.fresh_ability_var();
    let v2 = infer.fresh_ability_var();

    assert!(matches!(v1, AbilitySet::Var(0)));
    assert!(matches!(v2, AbilitySet::Var(1)));
}

#[test]
fn test_apply_abilities() {
    let mut infer = Infer::new();
    let var = AbilitySet::var(0);
    let concrete = AbilitySet::from_abilities([1, 2]);

    // Unify the variable with concrete
    infer.unify_abilities(&var, &concrete, span()).unwrap();

    // Apply should resolve the variable
    let applied = infer.apply_abilities(&var);
    assert_eq!(applied, concrete);

    // Applying to an unbound variable returns the variable
    let unbound = AbilitySet::var(99);
    let applied_unbound = infer.apply_abilities(&unbound);
    assert_eq!(applied_unbound, unbound);
}

#[test]
fn test_generalize_with_ability_vars() {
    let infer = Infer::new();
    let env = TypeEnv::new();

    // A function type with an ability variable
    let ty = Type::function_with_abilities(vec![Type::var(0)], Type::var(0), AbilitySet::var(1));

    let scheme = infer.generalize(&env, &ty);

    // Both the type variable and ability variable should be quantified
    assert_eq!(scheme.vars, vec![0]);
    assert_eq!(scheme.ability_vars, vec![1]);
}

#[test]
fn test_instantiate_with_ability_vars() {
    let mut infer = Infer::new();

    // Use higher IDs in the scheme so that fresh vars will be different
    let scheme = Scheme::poly_with_abilities(
        vec![100],
        vec![100],
        Type::function_with_abilities(vec![Type::var(100)], Type::var(100), AbilitySet::var(100)),
    );

    let ty = infer.instantiate(&scheme);

    // Should get fresh type and ability variables (different from the scheme's 100s)
    if let Type::Function(f) = ty {
        assert!(matches!(f.params[0], Type::Var(TypeVar::Unbound(id)) if id != 100));
        assert!(matches!(f.abilities, AbilitySet::Var(id) if id != 100));
    } else {
        panic!("Expected function type");
    }
}

#[test]
fn test_unify_functions_with_abilities() {
    let mut infer = Infer::new();

    let f1 = Type::function_with_abilities(
        vec![Type::Number],
        Type::String,
        AbilitySet::from_abilities([1]),
    );

    let f2 = Type::function_with_abilities(
        vec![Type::Number],
        Type::String,
        AbilitySet::from_abilities([1]),
    );

    let result = infer.unify(&f1, &f2, span());
    assert!(result.is_ok());
}

#[test]
fn test_unify_functions_different_abilities_fails() {
    let mut infer = Infer::new();

    let f1 = Type::function_with_abilities(
        vec![Type::Number],
        Type::String,
        AbilitySet::from_abilities([1]),
    );

    let f2 = Type::function_with_abilities(
        vec![Type::Number],
        Type::String,
        AbilitySet::from_abilities([2]),
    );

    let result = infer.unify(&f1, &f2, span());
    assert!(result.is_err());
}

#[test]
fn test_unify_ability_values() {
    let mut infer = Infer::new();

    let av1 = Type::ability_value(Type::String, AbilitySet::single(1));
    let av2 = Type::ability_value(Type::String, AbilitySet::single(1));

    let result = infer.unify(&av1, &av2, span());
    assert!(result.is_ok());
}

#[test]
fn test_unify_ability_values_different_result_fails() {
    let mut infer = Infer::new();

    let av1 = Type::ability_value(Type::String, AbilitySet::single(1));
    let av2 = Type::ability_value(Type::Number, AbilitySet::single(1));

    let result = infer.unify(&av1, &av2, span());
    assert!(result.is_err());
}

#[test]
fn test_ability_name_to_id() {
    let infer = Infer::new();
    assert_eq!(infer.ability_name_to_id("Console"), Some(1));
    assert_eq!(infer.ability_name_to_id("Exception"), Some(2));
    assert_eq!(infer.ability_name_to_id("Time"), Some(3));
    assert_eq!(infer.ability_name_to_id("Random"), Some(4));
    assert_eq!(infer.ability_name_to_id("Async"), Some(5));
    assert_eq!(infer.ability_name_to_id("Unknown"), None);
}

#[test]
fn test_ability_error_display() {
    let err = TypeError::new(
        TypeErrorKind::AbilityMismatch {
            expected: AbilitySet::from_abilities([1]),
            actual: AbilitySet::from_abilities([2]),
        },
        (0, 10),
    );
    let msg = format!("{err}");
    assert!(msg.contains("ability mismatch"));

    let err2 = TypeError::new(
        TypeErrorKind::UnknownAbility { name: "Foo".into() },
        (0, 10),
    );
    let msg2 = format!("{err2}");
    assert!(msg2.contains("unknown ability"));
    assert!(msg2.contains("Foo"));
}

#[test]
fn test_resolve_holes_simple() {
    let mut infer = Infer::new();

    // Hole becomes a fresh type variable
    let resolved = infer.resolve_holes(&Type::Hole);
    assert!(matches!(resolved, Type::Var(TypeVar::Unbound(_))));
}

#[test]
fn test_resolve_holes_nested() {
    let mut infer = Infer::new();

    // Holes in nested types get resolved
    let func = Type::function(vec![Type::Hole], Type::Hole);
    let resolved = infer.resolve_holes(&func);

    if let Type::Function(f) = resolved {
        assert!(matches!(f.params[0], Type::Var(TypeVar::Unbound(_))));
        assert!(matches!(*f.ret, Type::Var(TypeVar::Unbound(_))));
    } else {
        panic!("Expected function type");
    }
}

#[test]
fn test_resolve_holes_partial() {
    let mut infer = Infer::new();

    // Mix of concrete types and holes
    let tuple = Type::Tuple(vec![Type::Number, Type::Hole, Type::String]);
    let resolved = infer.resolve_holes(&tuple);

    if let Type::Tuple(elems) = resolved {
        assert_eq!(elems[0], Type::Number);
        assert!(matches!(elems[1], Type::Var(TypeVar::Unbound(_))));
        assert_eq!(elems[2], Type::String);
    } else {
        panic!("Expected tuple type");
    }
}

#[test]
fn test_require_ability_with_registry() {
    use crate::types::{AbilityInfo, AbilityRegistry};

    let mut registry = AbilityRegistry::new();

    // IO is ability 1
    registry.register(1, AbilityInfo::new("IO"));

    // FileSystem (2) depends on IO (1)
    registry.register(2, AbilityInfo::new("FileSystem").with_dependency(1));

    let mut infer = Infer::with_registry(registry);

    // When we require FileSystem, IO should also be required
    infer.require_ability(2);

    let abilities = infer.current_abilities();
    if let AbilitySet::Concrete(ids) = abilities {
        assert!(ids.contains(&1), "IO should be required");
        assert!(ids.contains(&2), "FileSystem should be required");
    } else {
        panic!("Expected concrete ability set");
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Async type checking tests (Milestone 9)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_async_all_type_inference() {
    let mut infer = Infer::new();

    // Create argument type: List<Ability<string, Console!>>
    let ability_value = Type::ability_value(Type::String, AbilitySet::single(1)); // Console = 1
    let list_of_abilities = Type::named("List", vec![ability_value]);

    // Look up Async.all with this argument
    let result = infer.lookup_ability_method("Async", "all", &[list_of_abilities], span());
    assert!(
        result.is_ok(),
        "Async.all should accept List<Ability<T, A!>>"
    );

    let (ability_id, result_ty, additional_abilities) = result.unwrap();

    // Should return Async ability ID
    assert_eq!(ability_id, 5, "Should return Async ability ID");

    // Should return List<string> (the result type wrapped in List)
    if let Type::Named(named) = &result_ty {
        assert_eq!(named.name.as_ref(), "List");
        assert_eq!(named.args.len(), 1);
        assert_eq!(named.args[0], Type::String);
    } else {
        panic!("Expected Named type List<string>, got {:?}", result_ty);
    }

    // Should include Console in additional abilities
    assert!(
        matches!(&additional_abilities, AbilitySet::Concrete(ids) if ids.contains(&1)),
        "Should include Console ability in additional_abilities"
    );
}

#[test]
fn test_async_race_type_inference() {
    let mut infer = Infer::new();

    // Create argument type: List<Ability<number, Time!>>
    let ability_value = Type::ability_value(Type::Number, AbilitySet::single(3)); // Time = 3
    let list_of_abilities = Type::named("List", vec![ability_value]);

    // Look up Async.race with this argument
    let result = infer.lookup_ability_method("Async", "race", &[list_of_abilities], span());
    assert!(
        result.is_ok(),
        "Async.race should accept List<Ability<T, A!>>"
    );

    let (ability_id, result_ty, additional_abilities) = result.unwrap();

    // Should return Async ability ID
    assert_eq!(ability_id, 5, "Should return Async ability ID");

    // Should return number (the unwrapped result type)
    assert_eq!(
        result_ty,
        Type::Number,
        "Async.race should return T, not List<T>"
    );

    // Should include Time in additional abilities
    assert!(
        matches!(&additional_abilities, AbilitySet::Concrete(ids) if ids.contains(&3)),
        "Should include Time ability in additional_abilities"
    );
}

#[test]
fn test_async_all_with_type_variable() {
    let mut infer = Infer::new();

    // Create a type variable for the result type
    let result_var = infer.fresh();
    let ability_var = infer.fresh_ability_var();

    // Create argument type: List<Ability<T, A!>> with fresh variables
    let ability_value = Type::ability_value(result_var.clone(), ability_var.clone());
    let list_of_abilities = Type::named("List", vec![ability_value]);

    // Look up Async.all - should succeed with polymorphic types
    let result = infer.lookup_ability_method("Async", "all", &[list_of_abilities], span());
    assert!(result.is_ok(), "Async.all should work with type variables");

    let (_, result_ty, _) = result.unwrap();

    // Result should be List<T> where T is the same variable
    if let Type::Named(named) = &result_ty {
        assert_eq!(named.name.as_ref(), "List");
        // The inner type should be related to our original type variable
        // (either the same or unified)
    } else {
        panic!("Expected Named type, got {:?}", result_ty);
    }
}

#[test]
fn test_async_all_wrong_arity() {
    let mut infer = Infer::new();

    // Try calling Async.all with no arguments
    let result = infer.lookup_ability_method("Async", "all", &[], span());
    assert!(
        result.is_err(),
        "Async.all should require exactly one argument"
    );

    // Try calling Async.all with two arguments
    let arg1 = Type::named(
        "List",
        vec![Type::ability_value(Type::String, AbilitySet::single(1))],
    );
    let arg2 = Type::named(
        "List",
        vec![Type::ability_value(Type::Number, AbilitySet::single(1))],
    );
    let result = infer.lookup_ability_method("Async", "all", &[arg1, arg2], span());
    assert!(result.is_err(), "Async.all should not accept two arguments");
}

#[test]
fn test_async_race_wrong_arity() {
    let mut infer = Infer::new();

    // Try calling Async.race with no arguments
    let result = infer.lookup_ability_method("Async", "race", &[], span());
    assert!(
        result.is_err(),
        "Async.race should require exactly one argument"
    );
}

#[test]
fn test_async_all_wrong_type() {
    let mut infer = Infer::new();

    // Try calling Async.all with a non-List type (e.g., just a number)
    let result = infer.lookup_ability_method("Async", "all", &[Type::Number], span());
    assert!(
        result.is_err(),
        "Async.all should reject non-List arguments"
    );

    // Try calling Async.all with List<number> (not List<Ability<...>>)
    let list_of_numbers = Type::named("List", vec![Type::Number]);
    let result = infer.lookup_ability_method("Async", "all", &[list_of_numbers], span());
    assert!(result.is_err(), "Async.all should reject List<number>");
}

// ─────────────────────────────────────────────────────────────────────────
// Handler literal type checking tests (Milestone 13)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_handler_literal_console_print() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // { print(msg) => resume(()) }
    let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
        "print",
        vec![Param::new(1, "msg")],
        Expr::unit(), // resume(()) - simplified for test
    )]);

    let ty = infer.infer_expr(&env, &mut expr).unwrap();

    // Should infer Handler<Console>
    if let Type::Handler(handler_ty) = ty {
        assert_eq!(handler_ty.ability, 0x0001); // Console ability ID
    } else {
        panic!("Expected Handler type, got {:?}", ty);
    }
}

#[test]
fn test_handler_literal_exception_throw() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // { throw(err) => ... }
    let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
        "throw",
        vec![Param::new(1, "err")],
        Expr::unit(),
    )]);

    let ty = infer.infer_expr(&env, &mut expr).unwrap();

    // Should infer Handler<Exception>
    if let Type::Handler(handler_ty) = ty {
        assert_eq!(handler_ty.ability, 0x0002); // Exception ability ID
    } else {
        panic!("Expected Handler type, got {:?}", ty);
    }
}

#[test]
fn test_handler_literal_time_methods() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // { now() => resume(0.0), wait(duration) => resume(()) }
    let mut expr = Expr::handler_literal(vec![
        HandlerLiteralMethod::new("now", vec![], Expr::number(0.0)),
        HandlerLiteralMethod::new("wait", vec![Param::new(1, "duration")], Expr::unit()),
    ]);

    let ty = infer.infer_expr(&env, &mut expr).unwrap();

    // Should infer Handler<Time>
    if let Type::Handler(handler_ty) = ty {
        assert_eq!(handler_ty.ability, 0x0003); // Time ability ID
    } else {
        panic!("Expected Handler type, got {:?}", ty);
    }
}

#[test]
fn test_handler_literal_unknown_method() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // { unknown_method(x) => ... } - doesn't match any ability
    let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
        "unknown_method",
        vec![Param::new(1, "x")],
        Expr::unit(),
    )]);

    let result = infer.infer_expr(&env, &mut expr);
    assert!(
        result.is_err(),
        "Should fail when methods don't match any ability"
    );
}

#[test]
fn test_handler_literal_wrong_arity() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // { print(a, b) => ... } - Console.print takes 1 arg, not 2
    let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
        "print",
        vec![Param::new(1, "a"), Param::new(2, "b")],
        Expr::unit(),
    )]);

    let result = infer.infer_expr(&env, &mut expr);
    assert!(result.is_err(), "Should fail when arity doesn't match");

    // Check error message mentions arity
    if let Err(err) = result {
        let msg = format!("{}", err.kind);
        assert!(
            msg.contains("expects 1 parameters") || msg.contains("expected 1"),
            "Error should mention expected arity: {}",
            msg
        );
    }
}

#[test]
fn test_handler_literal_partial_handler() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // { print(msg) => ... } - only handles print, not println/eprint
    // This should be allowed (partial handlers can be composed)
    let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
        "print",
        vec![Param::new(1, "msg")],
        Expr::unit(),
    )]);

    let ty = infer.infer_expr(&env, &mut expr).unwrap();
    assert!(
        matches!(ty, Type::Handler(_)),
        "Partial handlers should be allowed"
    );
}

#[test]
fn test_handler_literal_method_body_type_checked() {
    let mut infer = Infer::new();
    let env = TypeEnv::new();

    // { print(msg) => msg + 1 } - body uses msg (should type-check)
    let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
        "print",
        vec![Param::new(1, "msg")],
        Expr::binary(BinaryOp::Add, Expr::local(1), Expr::number(1.0)),
    )]);

    // This should succeed - the parameter 'msg' is in scope
    let result = infer.infer_expr(&env, &mut expr);
    assert!(
        result.is_ok(),
        "Handler method body should type-check with params in scope"
    );
}

#[test]
fn test_infer_ability_from_methods_uniqueness() {
    let infer = Infer::new();

    // "print" exists in Console
    let methods: Vec<Arc<str>> = vec!["print".into()];
    let ability = infer.infer_ability_from_methods(&methods);
    assert_eq!(ability, Some(0x0001)); // Console

    // "throw" exists only in Exception
    let methods: Vec<Arc<str>> = vec!["throw".into()];
    let ability = infer.infer_ability_from_methods(&methods);
    assert_eq!(ability, Some(0x0002)); // Exception

    // "now" exists only in Time
    let methods: Vec<Arc<str>> = vec!["now".into()];
    let ability = infer.infer_ability_from_methods(&methods);
    assert_eq!(ability, Some(0x0003)); // Time
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
                err.kind,
                TypeErrorKind::TypeMismatch {
                    expected: Type::Number,
                    actual: Type::String
                }
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

#[test]
fn test_error_display_field_not_found() {
    let err = TypeError::new(
        TypeErrorKind::FieldNotFound {
            field: "missing".into(),
            record_ty: Type::record([("x", Type::Number)]),
        },
        (0, 10),
    );
    let msg = format!("{err}");
    assert!(msg.contains("missing") || msg.contains("field"));
}

#[test]
fn test_error_display_tuple_index_out_of_bounds() {
    let err = TypeError::new(
        TypeErrorKind::TupleIndexOutOfBounds {
            index: 5,
            tuple_ty: Type::Tuple(vec![Type::Number, Type::String]),
        },
        (0, 10),
    );
    let msg = format!("{err}");
    assert!(msg.contains("5") || msg.contains("out of bounds") || msg.contains("index"));
}

#[test]
fn test_error_display_not_a_function() {
    let err = TypeError::new(TypeErrorKind::NotAFunction { ty: Type::Number }, (0, 10));
    let msg = format!("{err}");
    assert!(msg.contains("not a function") || msg.contains("number"));
}

#[test]
fn test_error_display_non_boolean_condition() {
    let err = TypeError::new(
        TypeErrorKind::NonBooleanCondition { ty: Type::Number },
        (0, 10),
    );
    let msg = format!("{err}");
    assert!(msg.contains("condition") || msg.contains("bool"));
}

#[test]
fn test_error_display_arity_mismatch() {
    let err = TypeError::new(
        TypeErrorKind::ArityMismatch {
            expected: 2,
            actual: 1,
        },
        (0, 10),
    );
    let msg = format!("{err}");
    assert!(msg.contains("2") && msg.contains("1"));
}

#[test]
fn test_error_display_match_arm_type_mismatch() {
    let err = TypeError::new(
        TypeErrorKind::MatchArmTypeMismatch {
            first: Type::Number,
            arm: Type::String,
        },
        (0, 10),
    );
    let msg = format!("{err}");
    assert!(msg.contains("match") || msg.contains("arm"));
}

#[test]
fn test_error_display_undefined_variable() {
    let err = TypeError::new(
        TypeErrorKind::UndefinedVariable { name: "foo".into() },
        (0, 10),
    );
    let msg = format!("{err}");
    assert!(msg.contains("foo") || msg.contains("undefined"));
}

#[test]
fn test_error_display_missing_ability() {
    let err = TypeError::new(
        TypeErrorKind::MissingAbility {
            required: 1,
            available: AbilitySet::Empty,
        },
        (0, 10),
    );
    let msg = format!("{err}");
    assert!(msg.contains("ability") || msg.contains("missing") || msg.contains("require"));
}

#[test]
fn test_error_display_sandbox_ability_violation() {
    let err = TypeError::new(
        TypeErrorKind::SandboxAbilityViolation {
            ability: "FileSystem".into(),
            allowed: vec!["Console".into()],
        },
        (0, 10),
    );
    let msg = format!("{err}");
    assert!(msg.contains("sandbox") || msg.contains("FileSystem") || msg.contains("not allowed"));
}

#[test]
fn test_error_display_handler_missing_method() {
    let err = TypeError::new(
        TypeErrorKind::HandlerMissingMethod {
            ability: "Console".into(),
            method: "print".into(),
        },
        (0, 10),
    );
    let msg = format!("{err}");
    assert!(msg.contains("print") || msg.contains("missing") || msg.contains("Console"));
}

#[test]
fn test_error_display_infinite_type() {
    let err = TypeError::new(
        TypeErrorKind::InfiniteType {
            var: 0,
            ty: Type::function(vec![Type::var(0)], Type::var(0)),
        },
        (0, 10),
    );
    let msg = format!("{err}");
    assert!(msg.contains("infinite") || msg.contains("recursive") || msg.contains("occurs"));
}

#[test]
fn test_error_display_cannot_infer() {
    let err = TypeError::new(
        TypeErrorKind::CannotInfer {
            hint: "ambiguous record field access".into(),
        },
        (0, 10),
    );
    let msg = format!("{err}");
    assert!(msg.contains("cannot") || msg.contains("infer") || msg.contains("ambiguous"));
}

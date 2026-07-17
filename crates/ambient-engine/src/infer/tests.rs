//! Unit tests for the inference entry points and error rendering.

use super::*;
use crate::types::AbilityId;

/// A distinct, recognizable `AbilityId` for tests.
fn aid(n: u8) -> AbilityId {
    AbilityId::from_bytes([n; 32])
}

#[test]
fn test_type_error_display() {
    let err = TypeError::new(
        TypeErrorKind::TypeMismatch {
            expected: Type::number(),
            actual: Type::string(),
        },
        (0, 10),
    );
    let msg = format!("{err}");
    assert!(msg.contains("type mismatch"));
    assert!(msg.contains("Number"));
    assert!(msg.contains("String"));
}

#[test]
fn test_ability_error_display() {
    let err = TypeError::new(
        TypeErrorKind::AbilityMismatch {
            expected: AbilitySet::from_abilities([aid(1)]),
            actual: AbilitySet::from_abilities([aid(2)]),
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
fn test_error_display_field_not_found() {
    let err = TypeError::new(
        TypeErrorKind::FieldNotFound {
            field: "missing".into(),
            record_ty: Type::record([("x", Type::number())]),
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
            tuple_ty: Type::Tuple(vec![Type::number(), Type::string()]),
        },
        (0, 10),
    );
    let msg = format!("{err}");
    assert!(msg.contains('5') || msg.contains("out of bounds") || msg.contains("index"));
}

#[test]
fn test_error_display_not_a_function() {
    let err = TypeError::new(TypeErrorKind::NotAFunction { ty: Type::number() }, (0, 10));
    let msg = format!("{err}");
    assert!(msg.contains("not a function") || msg.contains("Number"));
}

#[test]
fn test_error_display_non_boolean_condition() {
    let err = TypeError::new(
        TypeErrorKind::NonBooleanCondition { ty: Type::number() },
        (0, 10),
    );
    let msg = format!("{err}");
    assert!(msg.contains("condition") || msg.contains("Bool"));
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
    assert!(msg.contains('2') && msg.contains('1'));
}

#[test]
fn test_error_display_match_arm_type_mismatch() {
    let err = TypeError::new(
        TypeErrorKind::MatchArmTypeMismatch {
            first: Type::number(),
            arm: Type::string(),
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
            required: aid(1),
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

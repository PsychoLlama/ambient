//! Compile-time evaluation of `const` initializers.
//!
//! A `const` is a mapping from an identifier to a single hashed primitive
//! value. The initializer must be a *literal*, and it is resolved once, when
//! the module is built, rather than recomputed on each reference. This module
//! is the shared source of truth for *which* initializers qualify: the type
//! checker uses it to reject non-literal `const`s, and the compiler uses it to
//! inline the value at every reference site. Keeping both on one predicate
//! means "what the checker accepts" can never drift from "what the compiler can
//! emit".

use std::sync::Arc;

use crate::ast::{Expr, ExprKind, UnaryOp};
use crate::types::Type;
use crate::value::Value;

/// The primitive value a `const` initializer denotes, or `None` if the
/// initializer is not a permitted `const` literal.
///
/// The permitted forms are deliberately minimal — we are starting small and
/// may widen this later. They are the primitive literals (`()`, booleans,
/// numbers, strings) plus a negated numeric literal (`-1`), which is a literal
/// in every meaningful sense. Identifiers, arithmetic, function calls, and
/// compound values are intentionally excluded.
#[must_use]
pub(crate) fn literal_value(expr: &Expr) -> Option<Value> {
    match &expr.kind {
        ExprKind::Unit => Some(Value::Unit),
        ExprKind::Bool(b) => Some(Value::Bool(*b)),
        ExprKind::Number(n) => Some(Value::Number(*n)),
        ExprKind::String(s) => Some(Value::String(Arc::new(s.to_string()))),
        // A leading `-` on a numeric literal folds to the negated literal, so
        // the const still denotes one primitive value rather than an operation.
        ExprKind::Unary(UnaryOp::Neg, inner) => match &inner.kind {
            ExprKind::Number(n) => Some(Value::Number(-n)),
            _ => None,
        },
        _ => None,
    }
}

/// The type a `const` initializer denotes, inferred directly from the
/// literal, or `None` if the initializer is not a permitted `const` literal.
///
/// Because a `const` value is always a primitive literal, its type is fully
/// determined by the value — no unification needed. This lets the type
/// annotation be optional: when omitted, the checker registers the const
/// under this type. Kept in lockstep with [`literal_value`] so "what the
/// checker types" can never drift from "what the compiler emits".
#[must_use]
pub(crate) fn literal_type(expr: &Expr) -> Option<Type> {
    match &expr.kind {
        ExprKind::Unit => Some(Type::Unit),
        ExprKind::Bool(_) => Some(Type::bool()),
        ExprKind::Number(_) => Some(Type::number()),
        ExprKind::String(_) => Some(Type::string()),
        ExprKind::Unary(UnaryOp::Neg, inner) => match &inner.kind {
            ExprKind::Number(_) => Some(Type::number()),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Expr;

    #[test]
    fn literal_type_matches_each_permitted_form() {
        assert_eq!(literal_type(&Expr::unit()), Some(Type::Unit));
        assert_eq!(literal_type(&Expr::bool(true)), Some(Type::bool()));
        assert_eq!(literal_type(&Expr::number(42.0)), Some(Type::number()));
        assert_eq!(literal_type(&Expr::string("hi")), Some(Type::string()));
    }

    #[test]
    fn literal_type_folds_negated_number() {
        let neg = Expr::new(
            ExprKind::Unary(UnaryOp::Neg, Box::new(Expr::number(1.0))),
            crate::ast::Span::default(),
        );
        assert_eq!(literal_type(&neg), Some(Type::number()));
    }

    #[test]
    fn literal_type_rejects_non_literals() {
        // A negated non-number and a bare identifier are not `const` literals,
        // so they have no inferable literal type (kept in lockstep with
        // `literal_value`, which also rejects them).
        let neg_bool = Expr::new(
            ExprKind::Unary(UnaryOp::Neg, Box::new(Expr::bool(true))),
            crate::ast::Span::default(),
        );
        assert_eq!(literal_type(&neg_bool), None);
        assert_eq!(literal_value(&neg_bool), None);
        assert_eq!(literal_type(&Expr::name("x")), None);
    }
}

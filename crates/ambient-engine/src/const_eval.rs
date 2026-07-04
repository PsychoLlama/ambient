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

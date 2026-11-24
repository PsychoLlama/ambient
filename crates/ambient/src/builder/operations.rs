//! Operations: an operator applied to operands. See
//! `docs/spec/operations.md`.

use crate::surface::{Expr, Operator};

fn binary(operator: Operator, left: Expr, right: Expr) -> Expr {
    operation(operator, [left, right])
}

fn operation<const N: usize>(operator: Operator, operands: [Expr; N]) -> Expr {
    Expr::Operation {
        operator,
        operands: operands.into(),
    }
}

#[must_use]
pub fn add(left: Expr, right: Expr) -> Expr {
    binary(Operator::Add, left, right)
}

#[must_use]
pub fn subtract(left: Expr, right: Expr) -> Expr {
    binary(Operator::Subtract, left, right)
}

#[must_use]
pub fn multiply(left: Expr, right: Expr) -> Expr {
    binary(Operator::Multiply, left, right)
}

/// Division: of integers, toward zero.
#[must_use]
pub fn divide(left: Expr, right: Expr) -> Expr {
    binary(Operator::Divide, left, right)
}

/// What is left of an integer after dividing it.
#[must_use]
pub fn remainder(left: Expr, right: Expr) -> Expr {
    binary(Operator::Remainder, left, right)
}

/// `base ** exponent`.
#[must_use]
pub fn power(base: Expr, exponent: Expr) -> Expr {
    binary(Operator::Power, base, exponent)
}

/// How long a string or a list is.
#[must_use]
pub fn length(value: Expr) -> Expr {
    operation(Operator::Length, [value])
}

/// An iterator that `step` steps: a function that takes nothing and
/// gives null, or an item and the iterator of the rest.
#[must_use]
pub fn iterator(step: Expr) -> Expr {
    operation(Operator::Iterator, [step])
}

/// A string's encoding, or a list of integers, as bytes.
#[must_use]
pub fn to_bytes(value: Expr) -> Expr {
    operation(Operator::ToBytes, [value])
}

/// An iterator over the members of a list.
#[must_use]
pub fn iterate(list: Expr) -> Expr {
    operation(Operator::Iterate, [list])
}

/// An iterator over the integers from `start` up to `end`, which it
/// leaves out.
#[must_use]
pub fn range(start: Expr, end: Expr) -> Expr {
    binary(Operator::Range, start, end)
}

/// What an iterator gives next: null, or an item and the iterator of
/// the rest.
#[must_use]
pub fn step(iterator: Expr) -> Expr {
    operation(Operator::Step, [iterator])
}

/// The member of a list at `position`, or null.
#[must_use]
pub fn get(list: Expr, position: Expr) -> Expr {
    binary(Operator::Get, list, position)
}

/// `list` with `member` after its others.
#[must_use]
pub fn push(list: Expr, member: Expr) -> Expr {
    binary(Operator::Push, list, member)
}

/// One string or list followed by another.
#[must_use]
pub fn join(left: Expr, right: Expr) -> Expr {
    binary(Operator::Join, left, right)
}

/// What lies between two positions of a string or a list, or null.
#[must_use]
pub fn slice(value: Expr, start: Expr, end: Expr) -> Expr {
    operation(Operator::Slice, [value, start, end])
}

/// A float without its fraction, or the integer a string is; null if
/// there is none.
#[must_use]
pub fn to_int(value: Expr) -> Expr {
    operation(Operator::ToInt, [value])
}

/// The float nearest an integer, or the float a string is, or null.
#[must_use]
pub fn to_float(value: Expr) -> Expr {
    operation(Operator::ToFloat, [value])
}

/// A number as a string.
#[must_use]
pub fn to_text(value: Expr) -> Expr {
    operation(Operator::ToText, [value])
}

/// Whether one value comes before another, is the same, or comes after
/// it, as -1, 0, or 1.
#[must_use]
pub fn order(left: Expr, right: Expr) -> Expr {
    binary(Operator::Order, left, right)
}

#[must_use]
pub fn less(left: Expr, right: Expr) -> Expr {
    binary(Operator::Less, left, right)
}

#[must_use]
pub fn equal(left: Expr, right: Expr) -> Expr {
    binary(Operator::Equal, left, right)
}

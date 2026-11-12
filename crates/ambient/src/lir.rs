//! The lowered form: what a backend translates. See `spec/backends.md`.
//!
//! It is first-order and structured. Every intermediate value is bound to a
//! local, and reference counting is explicit:
//!
//! - an [`Expr`] evaluates to a word its consumer owns;
//! - a [`Value`] is borrowed: reading one takes no reference;
//! - a [`Expr::Let`] owns its local and releases it when its body ends,
//!   however it ends.
//!
//! Any expression may unwind instead of producing a value. A backend
//! checks after each [`Op`] that can, and leaves through the enclosing
//! `Let`s so that each releases its local.

use ambient_runtime::Fault;

use crate::core::Primitive;

/// An operand: something already evaluated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    Local(usize),
    Null,
    Bool(bool),
    /// An integer small enough to be immediate.
    Int(i64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// The operand, as a word the consumer owns.
    Copy(Value),
    /// An integer of any size.
    Int(i64),
    /// A float, as its bits.
    Float(u64),
    /// Calls the function in a slot of the current definition.
    Call { slot: usize, args: Vec<Value> },
    Primitive {
        operation: Primitive,
        args: Vec<Value>,
    },
}

impl Op {
    /// Whether the operation can unwind instead of producing a value.
    #[must_use]
    pub fn unwinds(&self) -> bool {
        match self {
            Self::Copy(_) | Self::Int(_) | Self::Float(_) => false,
            Self::Call { .. } => true,
            Self::Primitive { operation, .. } => matches!(
                operation,
                Primitive::IntAdd | Primitive::IntSubtract | Primitive::IntMultiply
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Op(Op),
    /// Evaluates `value` into `local`, then `body`, then releases `local`.
    Let {
        local: usize,
        value: Box<Expr>,
        body: Box<Expr>,
    },
    /// `condition` is a boolean.
    If {
        condition: Value,
        then: Box<Expr>,
        otherwise: Box<Expr>,
    },
    /// Unwinds.
    Fault(Fault),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    /// How many parameters the function takes. They are locals `0..params`,
    /// and are borrowed from the caller.
    pub params: usize,
    /// How many locals the function uses, parameters included.
    pub locals: usize,
    pub body: Expr,
}

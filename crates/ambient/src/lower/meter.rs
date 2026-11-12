//! Metering: the charges a function makes when it is compiled to run
//! inside a budget. See `spec/budgets.md`.
//!
//! A charge goes before the operation it pays for, so that a budget that
//! cannot afford an operation never performs it. Charges are a property
//! of the lowered form, which every backend shares, so a run costs the
//! same on each.

use super::Lowering;
use crate::core::Primitive;
use crate::lir::{Expr, Op, Value};

/// What an operation costs.
enum Cost {
    Free,
    /// So many steps and words of allocation.
    Fixed(u64, u64),
    /// As many words as the operands it copies hold.
    Sized(Vec<Value>),
}

fn cost(op: &Op) -> Cost {
    let words = |count: usize| Cost::Fixed(0, count as u64 + 1);
    match op {
        // A step for each call; loops are calls, so nothing runs free.
        // Starting a task is a call to it.
        Op::Call { .. }
        | Op::CallValue { .. }
        | Op::Native { .. }
        | Op::All(_)
        | Op::First(_)
        | Op::Spawn { .. } => Cost::Fixed(1, 0),
        Op::Sequence { members, .. } => words(members.len()),
        Op::Record(fields) => words(fields.len() * 2),
        Op::Closure { captures, .. } => words(captures.len() + 2),
        Op::Merge { base, overlay } => Cost::Sized(vec![*base, *overlay]),
        Op::Int(_) | Op::Float(_) | Op::Token | Op::Group => words(1),
        Op::Cell { .. } => words(2),
        Op::Hold => words(3),
        // An error: its record, and its empty trace.
        Op::Contain => words(9),
        Op::Primitive { operation, .. } => match operation {
            Primitive::Equal
            | Primitive::IntLess
            | Primitive::FloatLess
            | Primitive::CellRead
            | Primitive::CellReplace
            | Primitive::CellWait => Cost::Free,
            // A number that does not fit in a word is an object.
            _ => words(1),
        },
        _ => Cost::Free,
    }
}

impl Lowering {
    /// `body` with a charge before every operation that costs something.
    pub(super) fn meter(&mut self, body: Expr) -> Expr {
        let boxed = |lowering: &mut Self, expr: Box<Expr>| Box::new(lowering.meter(*expr));
        match body {
            Expr::Op(op) => {
                let charges = match cost(&op) {
                    Cost::Free => Vec::new(),
                    Cost::Fixed(steps, words) => vec![Op::Charge { steps, words }],
                    Cost::Sized(values) => values.into_iter().map(Op::ChargeSize).collect(),
                };
                charges
                    .into_iter()
                    .rev()
                    .fold(Expr::Op(op), |rest, charge| Expr::Let {
                        local: self.temporary(),
                        value: Box::new(Expr::Op(charge)),
                        body: Box::new(rest),
                    })
            }
            Expr::Let { local, value, body } => Expr::Let {
                local,
                value: boxed(self, value),
                body: boxed(self, body),
            },
            Expr::If {
                condition,
                then,
                otherwise,
            } => Expr::If {
                condition,
                then: boxed(self, then),
                otherwise: boxed(self, otherwise),
            },
            Expr::Try {
                body,
                recover,
                recovering,
            } => Expr::Try {
                body: boxed(self, body),
                recover: boxed(self, recover),
                recovering,
            },
            other => other,
        }
    }
}

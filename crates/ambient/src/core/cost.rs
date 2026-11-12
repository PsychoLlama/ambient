//! What evaluating a node costs inside a budget. See `spec/budgets.md`.
//!
//! Charges are defined here, on the core tree, and nowhere else. Lowering
//! makes the charge a node names before it evaluates the node, and no
//! other, so what a run costs does not change with how a node is
//! compiled.

use ambient_runtime::{SMALL_MAX, SMALL_MIN};

use super::{Expr, Primitive};
use crate::literal::Literal;

/// Steps, and words of allocation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    pub steps: u64,
    pub words: u64,
}

impl Cost {
    fn steps(steps: usize) -> Self {
        Self {
            steps: steps as u64,
            words: 0,
        }
    }

    /// An object that holds `count` words: they follow one more.
    fn object(count: usize) -> Self {
        Self {
            steps: 0,
            words: count as u64 + 1,
        }
    }

    fn and(self, other: Self) -> Self {
        Self {
            steps: self.steps + other.steps,
            words: self.words + other.words,
        }
    }
}

impl Expr {
    /// What the node costs, not counting the nodes inside it.
    ///
    /// A step for each call: a call, a perform, a task started, a cleanup
    /// run. Allocation for each value made, by its size. A merge costs
    /// besides as much as the records it merges hold, which is known only
    /// when it runs.
    #[must_use]
    pub fn cost(&self) -> Cost {
        match self {
            Self::Call { .. } | Self::Perform { .. } | Self::Spawn(_) => Cost::steps(1),
            // A task for each, and the tuple of what they give.
            Self::All(tasks) | Self::First(tasks) => {
                Cost::steps(tasks.len()).and(Cost::object(tasks.len()))
            }
            // The cleanup's call, and the unwinding it may have to hold.
            Self::Cleanup { .. } => Cost::steps(1).and(Cost::object(4)),
            Self::Tuple(members) | Self::List(members) => Cost::object(members.len()),
            Self::Record(fields) => Cost::object(fields.len() * 2),
            // Its definition and captures, and a record of the bindings
            // it captures, each with the budget it was bound inside.
            Self::Closure {
                captures, bindings, ..
            } => {
                let record = if bindings.is_empty() {
                    Cost::default()
                } else {
                    Cost::object(bindings.len() * 4)
                };
                Cost::object(captures.len() + 1).and(record)
            }
            Self::Cell { .. } => Cost::object(2),
            // A number that is not immediate is an object, and what
            // arithmetic gives may be one.
            Self::Literal(Literal::Int(value)) if !(SMALL_MIN..=SMALL_MAX).contains(value) => {
                Cost::object(1)
            }
            Self::Literal(Literal::Float(_))
            | Self::Group(_)
            | Self::Primitive {
                operation:
                    Primitive::IntAdd
                    | Primitive::IntSubtract
                    | Primitive::IntMultiply
                    | Primitive::FloatAdd
                    | Primitive::FloatSubtract
                    | Primitive::FloatMultiply,
                ..
            } => Cost::object(1),
            // The bindings it adds, each with its budget, and a token for
            // each handler that aborts.
            Self::Provide { handlers, .. } => {
                let aborting = handlers.iter().filter(|h| h.abort.is_some()).count();
                Cost::object(handlers.len() * 4 + aborting * 2)
            }
            // The error it gives if its body faults.
            Self::Boundary(_) => Cost::object(9),
            _ => Cost::default(),
        }
    }
}

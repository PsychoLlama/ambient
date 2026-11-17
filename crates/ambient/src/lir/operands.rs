//! The operands of an operation: every value it reads.

use super::{Op, Value};

impl Op {
    /// Each operand, to be rewritten in place.
    pub fn operands_mut(&mut self) -> Vec<&mut Value> {
        match self {
            Self::Int(_)
            | Self::Float(_)
            | Self::Slot(_)
            | Self::Token { .. }
            | Self::Traced
            | Self::Hold
            | Self::Enter { .. }
            | Self::Trace { .. }
            | Self::Contain
            | Self::Group
            | Self::Cleanup(_) => Vec::new(),
            Self::Copy(operand)
            | Self::Method(operand)
            | Self::Stepper(operand)
            | Self::Advance(operand)
            | Self::IsRecord(operand)
            | Self::Expire(operand)
            | Self::Caught(operand)
            | Self::Bounce(operand)
            | Self::Depth(operand)
            | Self::Rethrow(operand)
            | Self::All(operand)
            | Self::First(operand) => vec![operand],
            Self::Spawn {
                group: base,
                thunk: overlay,
            }
            | Self::GroupEnd {
                group: base,
                held: overlay,
            } => vec![base, overlay],
            Self::Merge {
                base,
                overlay,
                methods,
            } => vec![base, overlay, methods],
            Self::GroupClose { group, .. } => vec![group],
            Self::Abort { token, args } => vec![token, args],
            Self::Instantiate { function, types } => vec![function, types],
            Self::Native { bindings, args, .. } => vec![bindings, args],
            Self::Call { args, .. } | Self::Primitive { args, .. } | Self::Again(args) => {
                args.iter_mut().collect()
            }
            Self::CallValue { callee, args, .. } | Self::Tail { callee, args, .. } => {
                std::iter::once(callee).chain(args).collect()
            }
            Self::Closure { captures, .. } => captures.iter_mut().collect(),
            Self::Sequence { members, .. } => members.iter_mut().collect(),
            Self::Record(fields) => fields.iter_mut().map(|(_, member)| member).collect(),
            Self::Field { record, .. }
            | Self::Held { record, .. }
            | Self::Unbound { record, .. } => vec![record],
            Self::Member { tuple, .. } => vec![tuple],
            Self::HasField { value: subject, .. } | Self::IsTuple { value: subject, .. } => {
                vec![subject]
            }
            Self::Is {
                value: subject,
                scope,
                ..
            }
            | Self::Cell {
                value: subject,
                scope,
                ..
            } => vec![subject, scope],
            Self::Types(types) => types.iter_mut().map(|(_, within)| within).collect(),
        }
    }
}

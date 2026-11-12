//! The operands of an operation: every value it reads.

use super::{Op, Value};

impl Op {
    /// Each operand, to be rewritten in place.
    pub fn operands_mut(&mut self) -> Vec<&mut Value> {
        match self {
            Self::Int(_)
            | Self::Float(_)
            | Self::Slot(_)
            | Self::Token
            | Self::BudgetCurrent
            | Self::Charge { .. }
            | Self::Hold
            | Self::Trace { .. }
            | Self::Contain
            | Self::Group
            | Self::Cleanup(_) => Vec::new(),
            Self::Copy(operand)
            | Self::IsRecord(operand)
            | Self::Expire(operand)
            | Self::Caught(operand)
            | Self::Rethrow(operand)
            | Self::BudgetLeave(operand)
            | Self::BudgetSwitch(operand)
            | Self::BudgetBack { saved: operand, .. }
            | Self::BudgetCaught(operand)
            | Self::ChargeSize(operand)
            | Self::All(operand)
            | Self::First(operand) => vec![operand],
            Self::Spawn {
                group: base,
                thunk: overlay,
            }
            | Self::GroupEnd {
                group: base,
                held: overlay,
            }
            | Self::BudgetEnter {
                steps: base,
                words: overlay,
            }
            | Self::Merge { base, overlay } => vec![base, overlay],
            Self::GroupClose { group, .. } => vec![group],
            Self::Abort { token, args } => vec![token, args],
            Self::Native { bindings, args, .. } => vec![bindings, args],
            Self::Call { args, .. } | Self::Primitive { args, .. } => args.iter_mut().collect(),
            Self::CallValue { callee, args, .. } => std::iter::once(callee).chain(args).collect(),
            Self::Closure { captures, .. } => captures.iter_mut().collect(),
            Self::Sequence { members, .. } => members.iter_mut().collect(),
            Self::Record(fields) => fields.iter_mut().map(|(_, member)| member).collect(),
            Self::Field { record, .. } => vec![record],
            Self::Member { tuple, .. } => vec![tuple],
            Self::HasField { value: subject, .. }
            | Self::IsTuple { value: subject, .. }
            | Self::Is { value: subject, .. }
            | Self::Cell { value: subject, .. } => vec![subject],
        }
    }
}

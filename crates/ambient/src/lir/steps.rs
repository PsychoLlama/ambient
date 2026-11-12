//! Operations as calls into the runtime, and what a backend needs to know
//! of an operation besides: whether it can unwind, and whether its task
//! can suspend in it.

use ambient_runtime::Entry;

use super::{Cleanup, Op, Value};
use crate::core::Primitive;

/// One argument of a call into the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arg {
    Value(Value),
    /// A number that is not a value: a length, an index, a flag.
    Raw(u64),
    /// What the first step of the operation returned.
    Object,
    /// The definition of the closure in a slot.
    Definition(usize),
}

/// One call into the runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub entry: Entry,
    pub args: Vec<Arg>,
}

fn step(entry: Entry, args: impl IntoIterator<Item = Arg>) -> Step {
    Step {
        entry,
        args: args.into_iter().collect(),
    }
}

impl Op {
    /// Whether the operation can unwind instead of producing a value.
    #[must_use]
    pub fn unwinds(&self) -> bool {
        match self {
            Self::Call { .. }
            | Self::CallValue { .. }
            | Self::Abort { .. }
            | Self::Caught(_)
            | Self::Native { .. }
            | Self::All(_)
            | Self::First(_)
            | Self::Rethrow(_)
            | Self::Contain
            | Self::BudgetCaught(_)
            | Self::BudgetBack {
                unwinding: true, ..
            }
            | Self::Charge { .. }
            | Self::ChargeSize(_)
            | Self::GroupClose { .. }
            | Self::GroupEnd { .. }
            | Self::Cleanup(Cleanup::Failed) => true,
            Self::Primitive { operation, .. } => matches!(
                operation,
                Primitive::IntAdd
                    | Primitive::IntSubtract
                    | Primitive::IntMultiply
                    | Primitive::CellWait
            ),
            _ => false,
        }
    }

    /// Whether the task may suspend in the operation: it calls something
    /// that may, or waits.
    #[must_use]
    pub fn suspends(&self) -> bool {
        match self {
            Self::Call { .. }
            | Self::CallValue { .. }
            | Self::Native { .. }
            | Self::All(_)
            | Self::First(_)
            | Self::GroupClose { .. } => true,
            Self::Primitive { operation, .. } => matches!(operation, Primitive::CellWait),
            _ => false,
        }
    }

    /// The operation as calls into the runtime, if that is all it is. The
    /// first call returns the operation's result; the rest fill it in.
    #[must_use]
    pub fn steps(&self) -> Option<Vec<Step>> {
        use Arg::{Object, Raw};
        let value = |value: &Value| Arg::Value(*value);
        let key = |key: &usize| Arg::Value(Value::Constant(*key));
        let count = |length: usize| Raw(length as u64);
        Some(match self {
            Self::Copy(_)
            | Self::Int(_)
            | Self::Float(_)
            | Self::Slot(_)
            | Self::Call { .. }
            | Self::CallValue { .. } => return None,
            Self::Primitive { operation, args } => {
                vec![step(entry_for(*operation), args.iter().map(value))]
            }
            Self::Closure { slot, captures } => {
                let new = [Arg::Definition(*slot), count(captures.len())];
                let mut steps = vec![step(Entry::ClosureNew, new)];
                for (index, capture) in captures.iter().enumerate() {
                    let set = [Object, count(index), value(capture)];
                    steps.push(step(Entry::ClosureSet, set));
                }
                steps
            }
            Self::Sequence { list, members } => {
                let new = [Raw(u64::from(*list)), count(members.len())];
                let mut steps = vec![step(Entry::SequenceNew, new)];
                for (index, member) in members.iter().enumerate() {
                    let set = [Object, count(index), value(member)];
                    steps.push(step(Entry::SequenceSet, set));
                }
                steps
            }
            Self::Record(fields) => {
                let mut steps = vec![step(Entry::RecordNew, [count(fields.len())])];
                for (field, member) in fields {
                    steps.push(step(Entry::RecordSet, [Object, key(field), value(member)]));
                }
                steps
            }
            Self::Field { record, key: field } => {
                vec![step(Entry::RecordGet, [value(record), key(field)])]
            }
            Self::Member { tuple, index } => {
                vec![step(Entry::SequenceGet, [value(tuple), count(*index)])]
            }
            Self::HasField {
                value: subject,
                key: field,
            } => {
                vec![step(Entry::RecordHas, [value(subject), key(field)])]
            }
            Self::IsRecord(subject) => vec![step(Entry::RecordIs, [value(subject)])],
            Self::Merge { base, overlay } => {
                vec![step(Entry::RecordMerge, [value(base), value(overlay)])]
            }
            Self::Token => vec![step(Entry::TokenNew, [])],
            Self::Expire(token) => vec![step(Entry::TokenExpire, [value(token)])],
            Self::Abort { token, args } => {
                vec![step(Entry::Abort, [value(token), value(args)])]
            }
            Self::Caught(token) => vec![step(Entry::Caught, [value(token)])],
            Self::Hold
            | Self::Contain
            | Self::BudgetEnter { .. }
            | Self::BudgetLeave(_)
            | Self::BudgetCaught(_)
            | Self::BudgetCurrent
            | Self::BudgetSwitch(_)
            | Self::BudgetBack { .. }
            | Self::Charge { .. }
            | Self::ChargeSize(_)
            | Self::Group
            | Self::Spawn { .. }
            | Self::GroupClose { .. }
            | Self::GroupEnd { .. }
            | Self::Rethrow(_)
            | Self::Cleanup(_)
            | Self::All(_)
            | Self::First(_) => self.task_steps(),
            Self::Native {
                index,
                bindings,
                args,
            } => {
                vec![step(
                    Entry::Native,
                    [key(index), value(bindings), value(args)],
                )]
            }
            Self::Is { value: subject, ty } => vec![step(Entry::Is, [value(subject), key(ty)])],
            Self::IsTuple {
                value: subject,
                length,
            } => {
                vec![step(
                    Entry::SequenceIs,
                    [value(subject), Raw(0), count(*length)],
                )]
            }
        })
    }
}

impl Op {
    /// The steps of an operation on tasks, groups, or cleanups.
    fn task_steps(&self) -> Vec<Step> {
        use Arg::Raw;
        let value = |value: &Value| Arg::Value(*value);
        match self {
            Self::Hold => vec![step(Entry::Hold, [])],
            Self::Contain => vec![step(Entry::Contain, [])],
            Self::BudgetEnter { steps, words } => {
                vec![step(Entry::BudgetEnter, [value(steps), value(words)])]
            }
            Self::BudgetLeave(budget) => vec![step(Entry::BudgetLeave, [value(budget)])],
            Self::BudgetCurrent => vec![step(Entry::BudgetCurrent, [])],
            Self::BudgetSwitch(budget) => vec![step(Entry::BudgetSwitch, [value(budget)])],
            Self::BudgetBack { saved, unwinding } => {
                let unwinding = Raw(u64::from(*unwinding));
                vec![step(Entry::BudgetBack, [value(saved), unwinding])]
            }
            Self::BudgetCaught(budget) => vec![step(Entry::BudgetCaught, [value(budget)])],
            Self::Charge { steps, words } => {
                vec![step(Entry::Charge, [Raw(*steps), Raw(*words)])]
            }
            Self::ChargeSize(sized) => vec![step(Entry::ChargeSize, [value(sized)])],
            Self::Group => vec![step(Entry::GroupNew, [])],
            Self::Spawn { group, thunk } => {
                vec![step(Entry::Spawn, [value(group), value(thunk)])]
            }
            Self::GroupClose { group, unwinding } => {
                let unwinding = Raw(u64::from(*unwinding));
                vec![step(Entry::GroupClose, [value(group), unwinding])]
            }
            Self::GroupEnd { group, held } => {
                vec![step(Entry::GroupEnd, [value(group), value(held)])]
            }
            Self::Rethrow(held) => vec![step(Entry::Rethrow, [value(held)])],
            Self::Cleanup(moment) => {
                let entry = match moment {
                    Cleanup::Enter => Entry::CleanupEnter,
                    Cleanup::Leave => Entry::CleanupLeave,
                    Cleanup::Failed => Entry::CleanupFailed,
                };
                vec![step(entry, [])]
            }
            Self::All(thunks) => vec![step(Entry::All, [value(thunks)])],
            Self::First(thunks) => vec![step(Entry::First, [value(thunks)])],
            _ => Vec::new(),
        }
    }
}

/// The entry point that performs a primitive operation.
fn entry_for(operation: Primitive) -> Entry {
    match operation {
        Primitive::IntAdd => Entry::IntAdd,
        Primitive::IntSubtract => Entry::IntSubtract,
        Primitive::IntMultiply => Entry::IntMultiply,
        Primitive::IntLess => Entry::IntLess,
        Primitive::FloatAdd => Entry::FloatAdd,
        Primitive::FloatSubtract => Entry::FloatSubtract,
        Primitive::FloatMultiply => Entry::FloatMultiply,
        Primitive::FloatLess => Entry::FloatLess,
        Primitive::Equal => Entry::Equal,
        Primitive::CellNew => Entry::CellNew,
        Primitive::CellRead => Entry::CellRead,
        Primitive::CellReplace => Entry::CellReplace,
        Primitive::CellWait => Entry::CellWait,
    }
}

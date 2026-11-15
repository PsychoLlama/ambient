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
            | Self::Tail { .. }
            | Self::Bounce
            | Self::Abort { .. }
            | Self::Caught(_)
            | Self::Native { .. }
            | Self::All(_)
            | Self::First(_)
            | Self::Rethrow(_)
            | Self::Trace { .. }
            | Self::Contain
            | Self::BudgetCaught(_)
            | Self::BudgetBack {
                unwinding: true, ..
            }
            | Self::Charge { .. }
            | Self::ChargeSize(_)
            | Self::ChargeTasks(_)
            | Self::ChargeRead { .. }
            | Self::GroupClose { .. }
            | Self::GroupEnd { .. }
            | Self::Cleanup(Cleanup::Failed) => true,
            Self::Primitive { operation, .. } => matches!(
                operation,
                Primitive::IntAdd
                    | Primitive::IntSubtract
                    | Primitive::IntMultiply
                    | Primitive::IntDivide
                    | Primitive::IntRemainder
                    | Primitive::CellWait
                    | Primitive::Capture
            ),
            _ => false,
        }
    }

    /// Whether a fault can leave the operation: it can fault itself, or
    /// calls something that can. These are what a trace has frames for,
    /// with the operation that captures one, whose frame is its first.
    #[must_use]
    pub fn faults(&self) -> bool {
        match self {
            Self::Call { .. }
            | Self::CallValue { .. }
            | Self::Native { .. }
            | Self::Abort { .. }
            | Self::All(_)
            | Self::First(_) => true,
            Self::Primitive { operation, .. } => matches!(
                operation,
                Primitive::IntAdd
                    | Primitive::IntSubtract
                    | Primitive::IntMultiply
                    | Primitive::IntDivide
                    | Primitive::IntRemainder
                    | Primitive::Capture
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
            | Self::Bounce
            | Self::Native { waits: true, .. }
            | Self::All(_)
            | Self::First(_)
            | Self::GroupClose { .. } => true,
            Self::Primitive { operation, .. } => {
                matches!(operation, Primitive::CellWait | Primitive::Capture)
            }
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
            Self::Member { tuple, index } => {
                vec![step(Entry::SequenceGet, [value(tuple), count(*index)])]
            }
            Self::HasField { .. }
            | Self::IsRecord(_)
            | Self::IsTuple { .. }
            | Self::Is { .. }
            | Self::Cell { .. }
            | Self::Types(_) => self.type_steps(),
            Self::Record(_)
            | Self::Method(_)
            | Self::Instantiate { .. }
            | Self::Stepper(_)
            | Self::Advance(_)
            | Self::Field { .. }
            | Self::Held { .. }
            | Self::Unbound { .. }
            | Self::Merge { .. } => self.record_steps(),
            Self::Token => vec![step(Entry::TokenNew, [])],
            Self::Expire(token) => vec![step(Entry::TokenExpire, [value(token)])],
            Self::Abort { token, args } => {
                vec![step(Entry::Abort, [value(token), value(args)])]
            }
            Self::Caught(token) => vec![step(Entry::Caught, [value(token)])],
            Self::Tail {
                callee,
                args,
                metered,
            } => {
                // The call is pending from the first step, which is what
                // the operation gives; each argument then joins it.
                let metered = Raw(u64::from(*metered));
                let mut steps = vec![step(Entry::Tail, [value(callee), metered])];
                steps.extend(args.iter().map(|arg| step(Entry::TailArg, [value(arg)])));
                steps
            }
            Self::Bounce => vec![step(Entry::Bounce, [])],
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
            | Self::ChargeTasks(_)
            | Self::ChargeRead { .. }
            | Self::Group
            | Self::Spawn { .. }
            | Self::GroupClose { .. }
            | Self::GroupEnd { .. }
            | Self::Rethrow(_)
            | Self::Trace { .. }
            | Self::Cleanup(_)
            | Self::All(_)
            | Self::First(_) => self.task_steps(),
            Self::Native {
                index,
                waits,
                bindings,
                args,
            } => {
                let entry = [Entry::Native, Entry::NativeWait][usize::from(*waits)];
                vec![step(entry, [key(index), value(bindings), value(args)])]
            }
        })
    }
}

impl Op {
    /// The steps of an operation that makes or reads a record.
    fn record_steps(&self) -> Vec<Step> {
        use Arg::{Object, Raw};
        let value = |value: &Value| Arg::Value(*value);
        let key = |key: &usize| Arg::Value(Value::Constant(*key));
        let count = |length: usize| Raw(length as u64);
        match self {
            Self::Record(fields) => {
                let mut steps = vec![step(Entry::RecordNew, [count(fields.len())])];
                for (field, member) in fields {
                    steps.push(step(Entry::RecordSet, [Object, key(field), value(member)]));
                }
                steps
            }
            Self::Method(closure) => vec![step(Entry::Method, [value(closure)])],
            Self::Instantiate { function, types } => {
                vec![step(Entry::Instantiate, [value(function), value(types)])]
            }
            Self::Stepper(iterator) => vec![step(Entry::IteratorStepper, [value(iterator)])],
            Self::Advance(iterator) => vec![step(Entry::IteratorAdvance, [value(iterator)])],
            Self::Field { record, key: field } => {
                vec![step(Entry::RecordGet, [value(record), key(field)])]
            }
            Self::Held { record, key: field } => {
                vec![step(Entry::RecordHeld, [value(record), key(field)])]
            }
            Self::Unbound { record, key: field } => {
                vec![step(Entry::RecordMethod, [value(record), key(field)])]
            }
            Self::Merge {
                base,
                overlay,
                methods,
            } => {
                let merged = [value(base), value(overlay), value(methods)];
                vec![step(Entry::RecordMerge, merged)]
            }
            _ => Vec::new(),
        }
    }
}

impl Op {
    /// The steps of an operation that asks what a value is, says what a
    /// cell holds, or makes type arguments.
    fn type_steps(&self) -> Vec<Step> {
        use Arg::Raw;
        let value = |value: &Value| Arg::Value(*value);
        let key = |key: &usize| Arg::Value(Value::Constant(*key));
        match self {
            Self::HasField {
                value: subject,
                key: field,
            } => {
                vec![step(Entry::RecordHas, [value(subject), key(field)])]
            }
            Self::IsRecord(subject) => vec![step(Entry::RecordIs, [value(subject)])],
            Self::IsTuple {
                value: subject,
                length,
            } => {
                let length = Raw(*length as u64);
                vec![step(Entry::SequenceIs, [value(subject), Raw(0), length])]
            }
            Self::Is {
                value: subject,
                ty,
                scope,
            } => vec![step(Entry::Is, [value(subject), key(ty), value(scope)])],
            Self::Cell {
                value: held,
                ty,
                scope,
            } => vec![step(Entry::CellNew, [value(held), key(ty), value(scope)])],
            Self::Types(types) => {
                let mut steps = vec![step(Entry::ScopeNew, [Raw(types.len() as u64)])];
                for (index, (ty, within)) in types.iter().enumerate() {
                    let set = [Arg::Object, Raw(index as u64), key(ty), value(within)];
                    steps.push(step(Entry::ScopeSet, set));
                }
                steps
            }
            _ => Vec::new(),
        }
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
            Self::ChargeTasks(thunks) => vec![step(Entry::ChargeTasks, [value(thunks)])],
            Self::ChargeRead { record, key: field } => {
                vec![step(
                    Entry::ChargeRead,
                    [value(record), Arg::Value(Value::Constant(*field))],
                )]
            }
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
            Self::Trace {
                definition,
                position,
            } => {
                let definition = Arg::Value(Value::Constant(*definition));
                vec![step(Entry::Trace, [definition, Raw(*position as u64)])]
            }
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
        Primitive::CellRead => Entry::CellRead,
        Primitive::CellReplace => Entry::CellReplace,
        Primitive::CellWait => Entry::CellWait,
        Primitive::Capture => Entry::Capture,
        Primitive::IntDivide => Entry::IntDivide,
        Primitive::IntRemainder => Entry::IntRemainder,
        Primitive::FloatDivide => Entry::FloatDivide,
        Primitive::StringLess => Entry::StringLess,
        Primitive::StringLength => Entry::StringLength,
        Primitive::StringJoin => Entry::StringJoin,
        Primitive::StringSlice => Entry::StringSlice,
        Primitive::ListLength => Entry::ListLength,
        Primitive::ListGet => Entry::ListGet,
        Primitive::ListPush => Entry::ListPush,
        Primitive::ListJoin => Entry::ListJoin,
        Primitive::ListSlice => Entry::ListSlice,
        Primitive::IntFloat => Entry::IntFloat,
        Primitive::FloatInt => Entry::FloatInt,
        Primitive::IntText => Entry::IntText,
        Primitive::FloatText => Entry::FloatText,
        Primitive::StringInt => Entry::StringInt,
        Primitive::StringFloat => Entry::StringFloat,
        Primitive::StringCode => Entry::StringCode,
        Primitive::CodeString => Entry::CodeString,
        Primitive::IteratorNew => Entry::IteratorNew,
        Primitive::ListIterate => Entry::ListIterate,
        Primitive::StringIterate => Entry::StringIterate,
        Primitive::IntRange => Entry::IntRange,
        Primitive::BytesLength => Entry::BytesLength,
        Primitive::BytesGet => Entry::BytesGet,
        Primitive::BytesJoin => Entry::BytesJoin,
        Primitive::BytesSlice => Entry::BytesSlice,
        Primitive::StringBytes => Entry::StringBytes,
        Primitive::BytesText => Entry::BytesText,
        Primitive::ListBytes => Entry::ListBytes,
        Primitive::ListAll => Entry::All,
        Primitive::ListFirst => Entry::First,
    }
}

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
//!
//! Most operations are calls into the runtime. [`Op::steps`] spells those
//! out, so a backend translates calls and never decides what an operation
//! means.

use ambient_runtime::{Entry, Fault};

use crate::core::Primitive;

/// An operand: something already evaluated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    Local(usize),
    /// A value the closure being run captured, by position.
    Captured(usize),
    /// A constant of the definition being run, by position.
    Constant(usize),
    Null,
    Bool(bool),
    /// An integer small enough to be immediate.
    Int(i64),
}

/// Something a definition needs that is made once, when a program is
/// loaded, and never reclaimed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Constant {
    String(String),
    /// A field name, as the key records are indexed by.
    Field(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// The operand, as a word the consumer owns.
    Copy(Value),
    /// An integer of any size.
    Int(i64),
    /// A float, as its bits.
    Float(u64),
    /// The function in a slot of the current definition, as a value.
    Slot(usize),
    /// Calls the function in a slot of the current definition.
    Call { slot: usize, args: Vec<Value> },
    /// Calls a function value.
    CallValue { callee: Value, args: Vec<Value> },
    Primitive {
        operation: Primitive,
        args: Vec<Value>,
    },
    /// A closure over the lambda in a slot.
    Closure { slot: usize, captures: Vec<Value> },
    /// A tuple, or a list.
    Sequence { list: bool, members: Vec<Value> },
    /// A record. Each field is a [`Constant::Field`] and its value.
    Record(Vec<(usize, Value)>),
    /// A field of a record that has it.
    Field { record: Value, key: usize },
    /// A member of a tuple that has it.
    Member { tuple: Value, index: usize },
    /// Whether a value is a record with a field.
    HasField { value: Value, key: usize },
    /// Whether a value is a record.
    IsRecord(Value),
    /// Whether a value is a tuple of exactly this length.
    IsTuple { value: Value, length: usize },
}

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
            Self::Call { .. } | Self::CallValue { .. } => true,
            Self::Primitive { operation, .. } => matches!(
                operation,
                Primitive::IntAdd | Primitive::IntSubtract | Primitive::IntMultiply
            ),
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
    /// How many slots its definition has. Its constants follow them.
    pub slots: usize,
    pub constants: Vec<Constant>,
    pub body: Expr,
}

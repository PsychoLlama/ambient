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
//! A task suspends by unwinding and resumes by running its functions
//! again (`spec/backends.md`). [`Expr::Point`] and [`Expr::Resumed`] are
//! how a function takes part: see [`crate::lower`].
//!
//! Most operations are calls into the runtime. [`Op::steps`] spells those
//! out, so a backend translates calls and never decides what an operation
//! means.

mod operands;

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
    /// A tag: its package and itself, as [`crate::literal::Literal::tag_text`]
    /// spells them.
    Tag(String),
    /// A field name, as the key records are indexed by.
    Field(String),
    /// A type, as the descriptor a value is tested against. Its slots are
    /// those of the definition it is a constant of.
    Type(crate::core::Type),
    /// The ability in a slot, as the key bindings hold the budget its
    /// handler was bound inside under. Only metered code has these.
    AbilityBudget(usize),
    /// The ability in a slot, as the key bindings are indexed by.
    Ability(usize),
    /// Where the definition, an extern, sits among the externs of the
    /// program it is loaded into. Not a value.
    Native,
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
    /// Whether a type, a [`Constant::Type`], describes a value.
    Is { value: Value, ty: usize },
    /// A record with the fields of both, `overlay`'s where both have one.
    /// `base` may be null, for no fields. This is how bindings are
    /// extended.
    Merge { base: Value, overlay: Value },
    /// A new token: what a provide binds an abort handler to.
    Token,
    /// Expires a token, and is null.
    Expire(Value),
    /// Unwinds to the provide that holds `token`, with `args`, a tuple.
    Abort { token: Value, args: Value },
    /// For a provide whose body unwound: expires `token`, then is the
    /// arguments if the unwinding was an abort to it, and goes on
    /// unwinding if not.
    Caught(Value),
    /// For a block that unwound and has a cleanup to run: ends the
    /// unwinding, and is it, as a value to go on with later.
    Hold,
    /// Goes on with an unwinding that was held.
    Rethrow(Value),
    /// Entering or leaving a cleanup whose block completed. Leaving one
    /// that failed unwinds.
    Cleanup(Cleanup),
    /// For a boundary whose body unwound: if the unwinding is a fault,
    /// ends it and is the error that describes it; if not, goes on
    /// unwinding.
    Contain,
    /// Enters a budget of so many steps and words, both integers, and
    /// is the budget.
    BudgetEnter { steps: Value, words: Value },
    /// Leaves a budget whose body completed, and is null.
    BudgetLeave(Value),
    /// For a budget whose body unwound: leaves it, and is the limit that
    /// ran out if one did; if not, goes on unwinding.
    BudgetCaught(Value),
    /// The budget the task is inside, or null.
    BudgetCurrent,
    /// Puts the task inside a budget, as [`Op::BudgetCurrent`] gave it,
    /// and is the one it was inside.
    BudgetSwitch(Value),
    /// Puts the task back inside a budget it was switched out of, and is
    /// null; or, with `unwinding`, goes on unwinding.
    BudgetBack { saved: Value, unwinding: bool },
    /// Charges steps and words of allocation to the budgets the task is
    /// inside, and is null. Unwinds if one runs out.
    Charge { steps: u64, words: u64 },
    /// Charges for an allocation about as large as a value.
    ChargeSize(Value),
    /// A group with no tasks yet.
    Group,
    /// Starts a closure as a task of a group, and is null.
    Spawn { group: Value, thunk: Value },
    /// Waits for the tasks of a group, cancelling them first if the
    /// group's body unwound.
    GroupClose { group: Value, unwinding: bool },
    /// Ends a group whose tasks are over: unwinds if one failed, or with
    /// the unwinding `held`, which the body left by, if it is not null.
    GroupEnd { group: Value, held: Value },
    /// Runs each of the closures in a tuple as a task, and is the tuple of
    /// their results.
    All(Value),
    /// Runs each closure of a tuple as a task, and is the result of the
    /// first that is over.
    First(Value),
    /// Runs native code: the extern at `index`, a [`Constant::Native`],
    /// with the caller's bindings and `args`, a tuple.
    Native {
        index: usize,
        bindings: Value,
        args: Value,
    },
}

/// A moment in the cleanup of a block that completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cleanup {
    Enter,
    Leave,
    /// The cleanup unwound.
    Failed,
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
    /// `body`, or, if it unwinds, `recover`, which may go on unwinding.
    ///
    /// `recovering` is the points in `recover`, first and one past last:
    /// a function resuming to one of them goes straight to `recover`.
    Try {
        body: Box<Expr>,
        recover: Box<Expr>,
        recovering: (usize, usize),
    },
    /// An operation its task may suspend in, numbered within its
    /// function. If the operation unwinds because the task is
    /// suspending, the function saves its locals and returns. A function
    /// resuming to this point runs the operation again, which resumes
    /// whatever it called.
    Point {
        point: usize,
        op: Op,
    },
    /// The value a `Let` binds to `local`, in a function that may be
    /// resuming. If it is, and the point it is going back to is not among
    /// `points`, the value was computed before the task suspended and
    /// `local` already holds it.
    Resumed {
        points: (usize, usize),
        local: usize,
        value: Box<Expr>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    /// How many parameters the function takes, counting the bindings it is
    /// handed, which come first. They are locals `0..params`, and are
    /// borrowed from the caller.
    pub params: usize,
    /// How many locals the function uses, parameters included.
    pub locals: usize,
    /// How many slots its definition has. Its constants follow them.
    pub slots: usize,
    pub constants: Vec<Constant>,
    /// Whether the function enters a budget: a program with one that
    /// does is compiled metered.
    pub budgets: bool,
    pub body: Expr,
}

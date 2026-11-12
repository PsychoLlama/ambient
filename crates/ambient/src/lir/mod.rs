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
mod steps;

use ambient_runtime::Fault;

pub use steps::{Arg, Step};

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
    /// The hash of the definition itself, as a string: what a frame of a
    /// trace names it by. Known only once the program is linked.
    Own,
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
    /// Calls the function in a slot of the current definition. With
    /// `metered`, the call is made inside a budget, and reaches the code
    /// that charges for what it does.
    Call {
        slot: usize,
        args: Vec<Value>,
        metered: bool,
    },
    /// Calls a function value, as [`Op::Call`] calls a slot.
    CallValue {
        callee: Value,
        args: Vec<Value>,
        metered: bool,
    },
    Primitive {
        operation: Primitive,
        args: Vec<Value>,
    },
    /// A closure over the lambda in a slot.
    Closure { slot: usize, captures: Vec<Value> },
    /// A new cell that holds a value, made at a type, a
    /// [`Constant::Type`], if the type is known here.
    Cell { value: Value, ty: Option<usize> },
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
    /// For an operation that unwound: if the unwinding is a fault, adds
    /// a frame to its trace, for `position` of the definition, a
    /// [`Constant::Own`]. Goes on unwinding either way.
    Trace { definition: usize, position: usize },
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
    ///
    /// `frame` is what a trace says of the function while it is in the
    /// operation: its definition, a [`Constant::Own`], and a position
    /// in it.
    Point {
        point: usize,
        op: Op,
        frame: Option<(usize, usize)>,
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
    pub body: Expr,
}

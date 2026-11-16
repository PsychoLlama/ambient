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

pub use ambient_host::parts::Constant;
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

impl Value {
    /// Whether the operand may be an object, so that owning it takes a
    /// reference. A constant may be: one of loaded code is reclaimed
    /// with the last reference to it.
    #[must_use]
    pub fn counted(self) -> bool {
        matches!(self, Self::Local(_) | Self::Captured(_) | Self::Constant(_))
    }
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
    /// Leaves the function to call a function value with `args`, as
    /// [`Op::CallValue`] would: always unwinds, and whatever called the
    /// function makes the call.
    Tail {
        callee: Value,
        args: Vec<Value>,
        metered: bool,
    },
    /// For a call that unwound: if what it called left to make a call,
    /// makes that call and is its result; if not, goes on unwinding.
    /// It is handed how deep the function is: see [`Op::Enter`].
    Bounce(Value),
    /// Starts the function again with these as its parameters, the
    /// bindings it is handed first: what a call of itself that is the
    /// last thing it does comes to. Leaves for the [`Expr::Loop`] it
    /// is in, which takes the turn.
    Again(Vec<Value>),
    /// How deep the function is, which weighs `weight` words, as a
    /// function that makes calls starts: unwinds if that is too deep.
    Enter { weight: u64 },
    /// Says how deep the function is, before a call it makes: what it
    /// calls counts from there.
    Depth(Value),
    Primitive {
        operation: Primitive,
        args: Vec<Value>,
    },
    /// A closure over the lambda in a slot.
    Closure { slot: usize, captures: Vec<Value> },
    /// A new cell that holds a value, made at a type, a
    /// [`Constant::Type`], read in `scope`: the type arguments of the
    /// function being run, or null.
    Cell {
        value: Value,
        ty: usize,
        scope: Value,
    },
    /// Type arguments for a generic function: each a type, a
    /// [`Constant::Type`], and the type arguments it is read in, or null
    /// if it mentions no type parameter.
    Types(Vec<(usize, Value)>),
    /// A tuple, or a list.
    Sequence { list: bool, members: Vec<Value> },
    /// A record. Each field is a [`Constant::Field`] and its value.
    Record(Vec<(usize, Value)>),
    /// A closure, which nothing else holds, as a method of the record
    /// it is about to be a field of.
    Method(Value),
    /// A generic function at `types`, type arguments as a closure
    /// holds them, which follow any it holds already.
    Instantiate { function: Value, types: Value },
    /// The function that steps an iterator, or null if the runtime
    /// steps it: see [`Op::Advance`].
    Stepper(Value),
    /// A step of an iterator the runtime steps: null, or an item and
    /// the iterator of the rest.
    Advance(Value),
    /// A field of a record that has it: its value, or for a method the
    /// function bound to the record.
    Field { record: Value, key: usize },
    /// A member of a tuple that has it.
    Member { tuple: Value, index: usize },
    /// Whether a value is a record with a field.
    HasField { value: Value, key: usize },
    /// Whether a value is a record.
    IsRecord(Value),
    /// Whether a value is a tuple of exactly this length.
    IsTuple { value: Value, length: usize },
    /// Whether a type, a [`Constant::Type`], read in `scope` as a cell's
    /// is, describes a value.
    Is {
        value: Value,
        ty: usize,
        scope: Value,
    },
    /// What a record holds under a key it has, as it holds it: for a
    /// method, the method, bound to nothing.
    Held { record: Value, key: usize },
    /// The function of the method a record holds under a key: what a
    /// call of the method calls, with the record.
    Unbound { record: Value, key: usize },
    /// A record with the fields of both, `overlay`'s where both have one.
    /// `base` may be null, for no fields. This is how bindings are
    /// extended.
    ///
    /// `methods` is a tuple of the keys whose methods receive the new
    /// record, or null for none. Any other method stays bound to the
    /// record it is taken from.
    Merge {
        base: Value,
        overlay: Value,
        methods: Value,
    },
    /// A new token: what a provide binds an abort handler to. An abort
    /// to one that is `traced` gathers a trace as it unwinds.
    Token { traced: bool },
    /// The trace of the abort that was just caught, as the list its
    /// handler takes.
    Traced,
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
    /// Charges for the tasks a list of functions is run as.
    ChargeTasks(Value),
    /// Charges for what reading a key of a record allocates: a
    /// function, if a method is held there.
    ChargeRead { record: Value, key: usize },
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
    /// with the caller's bindings and `args`, a tuple. One that `waits`
    /// may suspend the task until the host answers.
    Native {
        index: usize,
        waits: bool,
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
    /// `body`, and `body` again each time an [`Op::Again`] in it says
    /// so, with what that gave as the function's parameters.
    ///
    /// `held` is the first of the locals the loop keeps them in, twice
    /// as many as the function has parameters and one: those the
    /// function has now, which the loop owns, then those of the turn
    /// to come, then whether a turn is. A parameter handed on as it is
    /// is in neither: nothing about it changes.
    Loop {
        body: Box<Expr>,
        held: usize,
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
    /// How many points its body has: places its task may suspend in.
    /// A function with none is never resumed, and is compiled without
    /// what resuming takes.
    pub points: usize,
    pub body: Expr,
}

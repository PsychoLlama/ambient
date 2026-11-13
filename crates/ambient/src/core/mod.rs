//! Core trees: the checked form of a definition. See `spec/core.md`.
//!
//! TODO: only the fragment the checker handles so far is here. The node set
//! in the spec is the target.

mod cost;
mod depth;
mod encode;
mod visit;

use serde::{Deserialize, Serialize};

use crate::literal::Literal;
use crate::symbol::Symbol;

pub use cost::Cost;
pub use depth::DEPTH;
pub use encode::Resolve;
pub(crate) use encode::encode_signature;

/// A reference to another definition, as an index into the slot table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Slot(pub usize);

/// What a record is keyed by.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Key {
    Field(String),
    /// The handler for an ability.
    Ability(Slot),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(remote = "Self")]
pub enum Type {
    Never,
    Unknown,
    Null,
    Bool,
    Int,
    Float,
    String,
    Tag,
    Literal(Literal),
    /// Fields come first, ordered by name; then handlers, ordered by
    /// their abilities' tags.
    Record(Vec<(Key, Type)>),
    /// A method, as a key of a record type holds one: what it needs of
    /// its receiver, and the function reading it gives.
    Method {
        needs: Box<Type>,
        function: Box<Type>,
    },
    Tuple(Vec<Type>),
    List(Box<Type>),
    Cell(Box<Type>),
    Iterator(Box<Type>),
    Function {
        params: Vec<Type>,
        ret: Box<Type>,
        /// Abilities, ordered by tag.
        row: Vec<Slot>,
    },
    /// Members are ordered by their encodings, without duplicates.
    Union(Vec<Type>),
    /// A type parameter of the enclosing definition, by position.
    Parameter(usize),
    /// A recursive type definition applied to arguments. A non-recursive
    /// one never appears: it is replaced by its expansion.
    Reference(Slot, Vec<Type>),
    /// A function type with type parameters of its own: the bound of
    /// each, and the function type.
    Generic {
        bounds: Vec<Type>,
        function: Box<Type>,
    },
    /// A type parameter of a generic function type this is inside,
    /// counted from the innermost.
    Bound(usize),
}

impl Type {
    /// The type of a function that steps an iterator of `member`: it
    /// takes nothing, and gives null or an item and the iterator of
    /// the rest.
    #[must_use]
    pub fn stepper(member: &Self) -> Self {
        let rest = Self::Iterator(Box::new(member.clone()));
        let step = Self::Tuple(vec![member.clone(), rest]);
        Self::Function {
            params: Vec::new(),
            ret: Box::new(Self::Union(vec![Self::Null, step])),
            row: Vec::new(),
        }
    }
}

/// An operation the language defines. A primitive has no slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Primitive {
    IntAdd,
    IntSubtract,
    IntMultiply,
    IntLess,
    FloatAdd,
    FloatSubtract,
    FloatMultiply,
    FloatLess,
    /// Structural equality, over every shape.
    Equal,
    CellRead,
    /// Puts a value in a cell if it holds the one given, and says whether
    /// it did.
    CellReplace,
    /// Suspends the task until a cell holds something other than the
    /// value given.
    CellWait,
    /// The trace of where the task is: a frame for this node and for
    /// each call it is inside, nearest first.
    Capture,
    IntDivide,
    IntRemainder,
    FloatDivide,
    /// Whether one string comes before another, by their bytes.
    StringLess,
    StringLength,
    StringJoin,
    /// The string between two positions, or null.
    StringSlice,
    ListLength,
    /// The member at a position, or null.
    ListGet,
    ListPush,
    ListJoin,
    /// The members between two positions, or null.
    ListSlice,
    /// The float nearest an integer.
    IntFloat,
    /// A float without its fraction, or null if that is no integer.
    FloatInt,
    IntText,
    FloatText,
    /// The integer a string is, or null.
    StringInt,
    /// The float a string is, or null.
    StringFloat,
    /// An iterator that a function steps.
    IteratorNew,
    /// An iterator over the members of a list.
    ListIterate,
    /// An iterator over the integers from one up to another.
    IntRange,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(remote = "Self")]
pub enum Pattern {
    Wildcard,
    Literal(Literal),
    Record(Vec<(String, Pattern)>),
    Tuple(Vec<Pattern>),
    Is(Type),
    /// Binds the next local to what the inner pattern matches. The local
    /// is numbered before any the inner pattern binds.
    Bind(Box<Pattern>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(remote = "Self")]
pub enum Expr {
    Null,
    Literal(Literal),
    Local(usize),
    /// A value the enclosing lambda captured, by position.
    Captured(usize),
    /// A function, with an argument for each of its type parameters.
    Reference(Slot, Vec<Type>),
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    /// Fields keep the order they were written in: it is their evaluation
    /// order.
    Record(Vec<(Key, Expr)>),
    /// A method of the record it is a field of. What it holds is a
    /// closure over the function that reads it: one that captures the
    /// method's function and then the record, which is null here and is
    /// the record once the field is read.
    Method(Box<Expr>),
    /// What an iterator gives next: null, or an item and the iterator
    /// of the rest. Not a primitive, since stepping may call a
    /// function. See `spec/iteration.md`.
    Step(Box<Expr>),
    Tuple(Vec<Expr>),
    List(Vec<Expr>),
    Field {
        target: Box<Expr>,
        key: Key,
    },
    /// A call of the method `target` holds under `key`: the method's
    /// function is called with the record and then `args`, so no
    /// function bound to the record is made.
    Send {
        target: Box<Expr>,
        key: Key,
        args: Vec<Expr>,
    },
    /// `left` with each of `keys` taken from `right` instead. The keys
    /// are those `right` is known to have: anything else it holds is left
    /// behind, because nothing is known of its type.
    ///
    /// `methods` are the keys the merged record holds a method under,
    /// by its type: each receives the merged record. Any other method
    /// either record holds stays bound to the record it had.
    Merge {
        left: Box<Expr>,
        right: Box<Expr>,
        keys: Vec<Key>,
        methods: Vec<Key>,
    },
    /// Binds the next local to `value` for the extent of `body`.
    Let {
        value: Box<Expr>,
        body: Box<Expr>,
    },
    Primitive {
        operation: Primitive,
        args: Vec<Expr>,
    },
    /// Takes the first arm whose pattern matches. Checking has shown that
    /// one always does.
    Match {
        subject: Box<Expr>,
        arms: Vec<(Pattern, Expr)>,
    },
    Cast {
        value: Box<Expr>,
        ty: Type,
        otherwise: Box<Expr>,
    },
    /// A function value: a lambda's definition, the values it captures, and
    /// the abilities whose bindings it captures.
    Closure {
        slot: Slot,
        type_args: Vec<Type>,
        captures: Vec<Expr>,
        bindings: Vec<Slot>,
    },
    /// A new cell that holds `value`, made to hold values of `ty`.
    Cell {
        ty: Type,
        value: Box<Expr>,
    },
    /// Evaluates `body`, then calls `cleanup`, a closure that takes
    /// nothing, however `body` ended. Is what `body` gave.
    Cleanup {
        cleanup: Box<Expr>,
        body: Box<Expr>,
    },
    /// Evaluates `body` within limits, both integers, and is what it
    /// gave, or the limit that ran out, as a tag.
    Budget {
        steps: Box<Expr>,
        allocation: Box<Expr>,
        body: Box<Expr>,
    },
    /// Evaluates the body, and is what it gave, or the fault that ended
    /// it, as an error.
    Boundary(Box<Expr>),
    /// Evaluates the body, and is what it gave once every task spawned in
    /// it is over.
    Group(Box<Expr>),
    /// Starts a closure that takes nothing as a task of the innermost
    /// group around it, and is null.
    Spawn(Box<Expr>),
    /// Runs each of these, closures that take nothing, as a task, and is
    /// the tuple of their results once every one is over.
    All(Vec<Expr>),
    /// Runs each of these as [`Self::All`] does, is the result of the
    /// first that is over, and cancels the rest.
    First(Vec<Expr>),
    /// What an observer is told an ability is: a string made from the
    /// ability's hash.
    AbilityId(Slot),
    /// Calls the handler bound to an ability. If the ability does not
    /// return, this unwinds to the provide that bound it.
    Perform {
        ability: Slot,
        args: Vec<Expr>,
        returns: bool,
    },
    /// Binds handlers for the extent of `body`. Handlers are evaluated in
    /// order, outside the bindings they make. With `only`, the body sees no
    /// other binding.
    Provide {
        handlers: Vec<Handler>,
        only: bool,
        body: Box<Expr>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handler {
    pub ability: Slot,
    /// A function value. For an ability that returns, it takes no
    /// abilities from its caller.
    pub handler: Expr,
    /// For an ability that does not return: how many arguments it takes.
    /// Its handler runs at the provide, and its result is the provide's.
    pub abort: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Function {
    /// The bound of each type parameter.
    pub type_params: Vec<Type>,
    /// The type of each captured value. Empty unless this is a lambda.
    pub captures: Vec<Type>,
    /// The abilities whose bindings it captured. Empty unless this is a
    /// lambda.
    pub bindings: Vec<Slot>,
    pub params: Vec<Type>,
    pub ret: Type,
    /// The abilities it takes from its caller, ordered by tag.
    pub row: Vec<Slot>,
    pub body: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tree {
    Type {
        /// How many type parameters the definition takes.
        params: usize,
        ty: Type,
    },
    Function(Function),
    Ability {
        /// The ID of the package that declares it, and its tag.
        package: String,
        tag: String,
        params: Vec<Type>,
        ret: Type,
    },
    /// A key of records: its package's ID, its tag, and the type of
    /// what a record holds under it.
    Key {
        package: String,
        tag: String,
        ty: Type,
    },
    /// A definition implemented in native code, found by its tag.
    Extern {
        package: String,
        tag: String,
        params: Vec<Type>,
        ret: Type,
        /// The ability it handles, if it is a native handler.
        handles: Option<Slot>,
        /// The abilities it performs, ordered by tag.
        row: Vec<Slot>,
    },
    /// A handler for each ability a world offers, ordered by the
    /// abilities' tags.
    Target {
        bindings: Vec<(Slot, Slot)>,
    },
}

/// A core definition whose slots are still symbols: what checking yields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Open {
    pub tree: Tree,
    /// What each slot refers to, in order of first use.
    pub slots: Vec<Symbol>,
    /// The lambdas written in this definition, each a definition of its
    /// own. A slot reaches one through its symbol.
    pub lambdas: Vec<Open>,
}

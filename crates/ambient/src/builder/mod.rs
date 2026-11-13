//! Builders: the first interface to the language. See `spec/trees.md`.
//!
//! Expressions and types are made with the free functions here and in
//! [`ty`]. They are assembled into definitions, and definitions into a
//! module. [`ModuleBuilder::build`] is where an ill-formed tree is rejected;
//! type errors are left to the checker.

mod definition;
mod error;
pub mod pat;
pub mod ty;
mod validate;
mod world;

pub use definition::{
    AbilityBuilder, FunctionBuilder, ModuleBuilder, ability, function, generic_type, module, type_,
};
pub use error::BuildError;
pub use world::{ExternBuilder, TargetBuilder, extern_function, native_handler, target};

use crate::literal::Literal;
use crate::surface::{Arm, CellOp, Expr, Key, Operator, Path, Pattern};

/// The first segment of a path into the module's own package.
pub const OWN_PACKAGE: &str = "pkg";

#[must_use]
pub fn null() -> Expr {
    Expr::Null
}

/// The trace of where this is evaluated: a frame for it and for each
/// call it is inside, nearest first.
#[must_use]
pub fn trace() -> Expr {
    Expr::Trace
}

#[must_use]
pub fn boolean(value: bool) -> Expr {
    Expr::Literal(Literal::Bool(value))
}

#[must_use]
pub fn int(value: i64) -> Expr {
    Expr::Literal(Literal::Int(value))
}

#[must_use]
pub fn float(value: f64) -> Expr {
    Expr::Literal(Literal::float(value))
}

#[must_use]
pub fn string(value: &str) -> Expr {
    Expr::Literal(Literal::String(value.to_owned()))
}

/// A tag, of the package it is written in.
#[must_use]
pub fn tag(tag: &str) -> Expr {
    Expr::Literal(Literal::Tag {
        package: String::new(),
        tag: tag.to_owned(),
    })
}

pub fn tuple(members: impl IntoIterator<Item = Expr>) -> Expr {
    Expr::Tuple(members.into_iter().collect())
}

pub fn list(members: impl IntoIterator<Item = Expr>) -> Expr {
    Expr::List(members.into_iter().collect())
}

fn binary(operator: Operator, left: Expr, right: Expr) -> Expr {
    operation(operator, [left, right])
}

fn operation<const N: usize>(operator: Operator, operands: [Expr; N]) -> Expr {
    Expr::Operation {
        operator,
        operands: operands.into(),
    }
}

#[must_use]
pub fn add(left: Expr, right: Expr) -> Expr {
    binary(Operator::Add, left, right)
}

#[must_use]
pub fn subtract(left: Expr, right: Expr) -> Expr {
    binary(Operator::Subtract, left, right)
}

#[must_use]
pub fn multiply(left: Expr, right: Expr) -> Expr {
    binary(Operator::Multiply, left, right)
}

/// Division: of integers, toward zero.
#[must_use]
pub fn divide(left: Expr, right: Expr) -> Expr {
    binary(Operator::Divide, left, right)
}

/// What is left of an integer after dividing it.
#[must_use]
pub fn remainder(left: Expr, right: Expr) -> Expr {
    binary(Operator::Remainder, left, right)
}

/// How long a string or a list is.
#[must_use]
pub fn length(value: Expr) -> Expr {
    operation(Operator::Length, [value])
}

/// The member of a list at `position`, or null.
#[must_use]
pub fn get(list: Expr, position: Expr) -> Expr {
    binary(Operator::Get, list, position)
}

/// `list` with `member` after its others.
#[must_use]
pub fn push(list: Expr, member: Expr) -> Expr {
    binary(Operator::Push, list, member)
}

/// One string or list followed by another.
#[must_use]
pub fn join(left: Expr, right: Expr) -> Expr {
    binary(Operator::Join, left, right)
}

/// What lies between two positions of a string or a list, or null.
#[must_use]
pub fn slice(value: Expr, start: Expr, end: Expr) -> Expr {
    operation(Operator::Slice, [value, start, end])
}

/// A float without its fraction, or the integer a string is; null if
/// there is none.
#[must_use]
pub fn to_int(value: Expr) -> Expr {
    operation(Operator::ToInt, [value])
}

/// The float nearest an integer, or the float a string is, or null.
#[must_use]
pub fn to_float(value: Expr) -> Expr {
    operation(Operator::ToFloat, [value])
}

/// A number as a string.
#[must_use]
pub fn to_text(value: Expr) -> Expr {
    operation(Operator::ToText, [value])
}

#[must_use]
pub fn less(left: Expr, right: Expr) -> Expr {
    binary(Operator::Less, left, right)
}

#[must_use]
pub fn equal(left: Expr, right: Expr) -> Expr {
    binary(Operator::Equal, left, right)
}

/// Takes the first arm whose pattern matches `subject`.
pub fn match_(subject: Expr, arms: impl IntoIterator<Item = (Pattern, Expr)>) -> Expr {
    Expr::Match {
        subject: Box::new(subject),
        arms: arms
            .into_iter()
            .map(|(pattern, body)| Arm { pattern, body })
            .collect(),
    }
}

/// A function written in place. A parameter given no type takes it from
/// where the lambda is used.
pub fn lambda<'a>(
    params: impl IntoIterator<Item = (&'a str, Option<crate::surface::Type>)>,
    body: Expr,
) -> Expr {
    lambda_row(params, [], body)
}

/// A lambda that says what it gives, so that it is known before its
/// body is checked.
pub fn lambda_returning<'a>(
    params: impl IntoIterator<Item = (&'a str, Option<crate::surface::Type>)>,
    returns: crate::surface::Type,
    body: Expr,
) -> Expr {
    match lambda(params, body) {
        Expr::Lambda {
            params, row, body, ..
        } => Expr::Lambda {
            params,
            row,
            returns: Some(returns),
            body,
        },
        other => other,
    }
}

/// A lambda that takes the abilities in `row` from its caller. Any other
/// ability it uses is captured where it is written.
pub fn lambda_row<'a>(
    params: impl IntoIterator<Item = (&'a str, Option<crate::surface::Type>)>,
    row: impl IntoIterator<Item = &'a str>,
    body: Expr,
) -> Expr {
    Expr::Lambda {
        params: params
            .into_iter()
            .map(|(name, ty)| (name.to_owned(), ty))
            .collect(),
        row: row.into_iter().map(Path::parse).collect(),
        returns: None,
        body: Box::new(body),
    }
}

/// Calls the handler bound to the ability at a dotted path.
pub fn perform(ability: &str, args: impl IntoIterator<Item = Expr>) -> Expr {
    Expr::Perform {
        ability: Path::parse(ability),
        args: args.into_iter().collect(),
    }
}

/// A record keyed by abilities, each a dotted path, with a handler for
/// each.
pub fn bundle<'a>(handlers: impl IntoIterator<Item = (&'a str, Expr)>) -> Expr {
    Expr::Record(
        handlers
            .into_iter()
            .map(|(ability, handler)| (Key::Ability(Path::parse(ability)), handler))
            .collect(),
    )
}

/// The handler a bundle holds for the ability at a dotted path.
#[must_use]
pub fn handler(bundle: Expr, ability: &str) -> Expr {
    Expr::Field {
        target: Box::new(bundle),
        key: Key::Ability(Path::parse(ability)),
    }
}

/// A record with the fields of both, `right`'s where both have one.
#[must_use]
pub fn merge(left: Expr, right: Expr) -> Expr {
    Expr::Merge {
        left: Box::new(left),
        right: Box::new(right),
    }
}

/// Binds the handlers in `bundle` for the extent of `body`, on top of what
/// is already bound.
#[must_use]
pub fn using(bundle: Expr, body: Expr) -> Expr {
    Expr::Provide {
        bundle: Box::new(bundle),
        only: false,
        body: Box::new(body),
    }
}

/// Binds the handlers in `bundle` for the extent of `body`, which sees no
/// other binding.
#[must_use]
pub fn using_only(bundle: Expr, body: Expr) -> Expr {
    Expr::Provide {
        bundle: Box::new(bundle),
        only: true,
        body: Box::new(body),
    }
}

/// Evaluates `body` within `steps` steps and `allocation` words of
/// allocation, and is what it gave, or the limit that ran out.
#[must_use]
pub fn budget(steps: Expr, allocation: Expr, body: Expr) -> Expr {
    Expr::Budget {
        steps: Box::new(steps),
        allocation: Box::new(allocation),
        body: Box::new(body),
    }
}

/// Evaluates `body`, and is what it gave, or the fault that ended it, as
/// an error.
#[must_use]
pub fn boundary(body: Expr) -> Expr {
    Expr::Boundary(Box::new(body))
}

/// Evaluates `body`, and is what it gave once every task it spawned is
/// over.
#[must_use]
pub fn group(body: Expr) -> Expr {
    Expr::Group(Box::new(body))
}

/// Starts `task` as a task of the group it is written in.
#[must_use]
pub fn spawn(task: Expr) -> Expr {
    Expr::Spawn(Box::new(task))
}

/// Evaluates `body`, and then `cleanup`, however `body` ended.
#[must_use]
pub fn cleanup(cleanup: Expr, body: Expr) -> Expr {
    Expr::Cleanup {
        cleanup: Box::new(cleanup),
        body: Box::new(body),
    }
}

/// Evaluates each expression as a task, and is the tuple of their results
/// once every one is over.
pub fn all(branches: impl IntoIterator<Item = Expr>) -> Expr {
    Expr::All(branches.into_iter().collect())
}

/// Evaluates each expression as a task, is the result of the first that
/// is over, and cancels the rest.
pub fn first(branches: impl IntoIterator<Item = Expr>) -> Expr {
    Expr::First(branches.into_iter().collect())
}

fn cell_op(operation: CellOp, ty: Option<crate::surface::Type>, args: Vec<Expr>) -> Expr {
    Expr::Cell {
        operation,
        ty,
        args,
    }
}

/// A new cell of values of type `ty`, holding `value`.
#[must_use]
pub fn cell(ty: crate::surface::Type, value: Expr) -> Expr {
    cell_op(CellOp::New, Some(ty), vec![value])
}

/// The value a cell holds.
#[must_use]
pub fn read(cell: Expr) -> Expr {
    cell_op(CellOp::Read, None, vec![cell])
}

/// Puts `new` in a cell if it still holds `seen`, and is whether it did.
#[must_use]
pub fn replace(cell: Expr, seen: Expr, new: Expr) -> Expr {
    cell_op(CellOp::Replace, None, vec![cell, seen, new])
}

/// Suspends the task until a cell holds something other than `seen`.
#[must_use]
pub fn wait(cell: Expr, seen: Expr) -> Expr {
    cell_op(CellOp::Wait, None, vec![cell, seen])
}

/// Evaluates `body`, calling `observer` with each ability it performs:
/// a record of the ability's id, the arguments, and the outcome.
#[must_use]
pub fn recording(observer: Expr, body: Expr) -> Expr {
    Expr::Observe {
        observer: Box::new(observer),
        body: Box::new(body),
    }
}

/// What an observer is told the ability at a dotted path is.
#[must_use]
pub fn ability_id(ability: &str) -> Expr {
    Expr::AbilityId(Path::parse(ability))
}

/// [`using`] a bundle written in place.
pub fn provide<'a>(handlers: impl IntoIterator<Item = (&'a str, Expr)>, body: Expr) -> Expr {
    using(bundle(handlers), body)
}

/// [`using_only`] a bundle written in place.
pub fn provide_only<'a>(handlers: impl IntoIterator<Item = (&'a str, Expr)>, body: Expr) -> Expr {
    using_only(bundle(handlers), body)
}

/// Runs `call`, a call to a named function, during compilation, and
/// stands for its result.
#[must_use]
pub fn comptime(call: Expr) -> Expr {
    Expr::Comptime(Box::new(call))
}

/// `value` at type `ty` if the type describes it, and `otherwise` if not.
#[must_use]
pub fn cast(value: Expr, ty: crate::surface::Type, otherwise: Expr) -> Expr {
    Expr::Cast {
        value: Box::new(value),
        ty,
        otherwise: Box::new(otherwise),
    }
}

/// A local or a definition named by a dotted path, such as `"x"` or
/// `"math.add"`.
#[must_use]
pub fn name(path: &str) -> Expr {
    Expr::Name(Path::parse(path))
}

/// A generic function with its type arguments written out.
pub fn instantiate(path: &str, args: impl IntoIterator<Item = crate::surface::Type>) -> Expr {
    Expr::Instantiate {
        function: Path::parse(path),
        args: args.into_iter().collect(),
    }
}

pub fn call(callee: Expr, args: impl IntoIterator<Item = Expr>) -> Expr {
    Expr::Call {
        callee: Box::new(callee),
        args: args.into_iter().collect(),
    }
}

/// A call of the method `method` of `target`: a field of a record, or
/// a function of the standard library for the shape `target` has.
pub fn send(target: Expr, method: &str, args: impl IntoIterator<Item = Expr>) -> Expr {
    Expr::Send {
        target: Box::new(target),
        method: Key::Field(method.to_owned()),
        args: args.into_iter().collect(),
        shapes: Vec::new(),
    }
}

pub fn record<'a>(fields: impl IntoIterator<Item = (&'a str, Expr)>) -> Expr {
    Expr::Record(
        fields
            .into_iter()
            .map(|(name, value)| (Key::Field(name.to_owned()), value))
            .collect(),
    )
}

/// `function` as a method of the record it is a field of: its first
/// parameter is the record the field is read from.
#[must_use]
pub fn method(function: Expr) -> Expr {
    Expr::Method(Box::new(function))
}

#[must_use]
pub fn field(target: Expr, name: &str) -> Expr {
    Expr::Field {
        target: Box::new(target),
        key: Key::Field(name.to_owned()),
    }
}

/// Binds `name` to `value` for the extent of `body`.
#[must_use]
pub fn let_(name: &str, value: Expr, body: Expr) -> Expr {
    Expr::Let {
        name: name.to_owned(),
        value: Box::new(value),
        body: Box::new(body),
    }
}

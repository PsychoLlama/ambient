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

pub use definition::{FunctionBuilder, ModuleBuilder, function, generic_type, module, type_};
pub use error::BuildError;

use crate::literal::Literal;
use crate::surface::{Arm, Expr, Operator, Path, Pattern};

/// The first segment of a path into the module's own package.
pub const OWN_PACKAGE: &str = "pkg";

#[must_use]
pub fn null() -> Expr {
    Expr::Null
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

pub fn tuple(members: impl IntoIterator<Item = Expr>) -> Expr {
    Expr::Tuple(members.into_iter().collect())
}

pub fn list(members: impl IntoIterator<Item = Expr>) -> Expr {
    Expr::List(members.into_iter().collect())
}

fn binary(operator: Operator, left: Expr, right: Expr) -> Expr {
    Expr::Binary {
        operator,
        left: Box::new(left),
        right: Box::new(right),
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
    Expr::Lambda {
        params: params
            .into_iter()
            .map(|(name, ty)| (name.to_owned(), ty))
            .collect(),
        body: Box::new(body),
    }
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

pub fn record<'a>(fields: impl IntoIterator<Item = (&'a str, Expr)>) -> Expr {
    Expr::Record(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}

#[must_use]
pub fn field(target: Expr, name: &str) -> Expr {
    Expr::Field {
        target: Box::new(target),
        name: name.to_owned(),
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

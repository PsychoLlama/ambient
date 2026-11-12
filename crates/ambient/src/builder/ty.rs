//! Type constructors.

use crate::literal::Literal;
use crate::surface::{Path, Type};

#[must_use]
pub fn never() -> Type {
    Type::Never
}

#[must_use]
pub fn unknown() -> Type {
    Type::Unknown
}

/// The type of one boolean.
#[must_use]
pub fn boolean(value: bool) -> Type {
    Type::Literal(Literal::Bool(value))
}

/// The type of one integer.
#[must_use]
pub fn integer(value: i64) -> Type {
    Type::Literal(Literal::Int(value))
}

/// The type of one string.
#[must_use]
pub fn text(value: &str) -> Type {
    Type::Literal(Literal::String(value.to_owned()))
}

pub fn tuple(members: impl IntoIterator<Item = Type>) -> Type {
    Type::Tuple(members.into_iter().collect())
}

#[must_use]
pub fn list(member: Type) -> Type {
    Type::List(Box::new(member))
}

pub fn union(members: impl IntoIterator<Item = Type>) -> Type {
    Type::Union(members.into_iter().collect())
}

#[must_use]
pub fn null() -> Type {
    Type::Null
}

#[must_use]
pub fn bool() -> Type {
    Type::Bool
}

#[must_use]
pub fn int() -> Type {
    Type::Int
}

#[must_use]
pub fn float() -> Type {
    Type::Float
}

#[must_use]
pub fn string() -> Type {
    Type::String
}

pub fn record<'a>(fields: impl IntoIterator<Item = (&'a str, Type)>) -> Type {
    Type::Record(
        fields
            .into_iter()
            .map(|(name, ty)| (name.to_owned(), ty))
            .collect(),
    )
}

/// A function that takes no abilities from its caller.
pub fn function(params: impl IntoIterator<Item = Type>, ret: Type) -> Type {
    function_row(params, [], ret)
}

/// A function that takes the abilities in `row`, each a dotted path, from
/// its caller.
pub fn function_row<'a>(
    params: impl IntoIterator<Item = Type>,
    row: impl IntoIterator<Item = &'a str>,
    ret: Type,
) -> Type {
    Type::Function {
        params: params.into_iter().collect(),
        ret: Box::new(ret),
        row: row.into_iter().map(Path::parse).collect(),
    }
}

/// A type named by a dotted path, such as `"Point"` or `"geometry.Point"`.
#[must_use]
pub fn named(path: &str) -> Type {
    Type::Named(Path::parse(path), Vec::new())
}

/// A generic type applied to arguments.
pub fn apply(path: &str, args: impl IntoIterator<Item = Type>) -> Type {
    Type::Named(Path::parse(path), args.into_iter().collect())
}

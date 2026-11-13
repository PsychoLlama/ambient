//! Type constructors.

use crate::literal::Literal;
use crate::surface::{Key, Path, Type};

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

/// Every tag value.
#[must_use]
pub fn tag() -> Type {
    Type::Tag
}

/// The type of one tag, of the package it is written in.
#[must_use]
pub fn tagged(tag: &str) -> Type {
    Type::Literal(Literal::Tag {
        package: String::new(),
        tag: tag.to_owned(),
    })
}

/// A cell that holds a value of type `member`.
#[must_use]
pub fn cell(member: Type) -> Type {
    Type::Cell(Box::new(member))
}

/// An iterator of items of a type.
#[must_use]
pub fn iterator(member: Type) -> Type {
    Type::Iterator(Box::new(member))
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
            .map(|(name, ty)| (Key::Field(name.to_owned()), ty))
            .collect(),
    )
}

/// A record keyed by abilities, each a dotted path, with the type of its
/// handler.
pub fn bundle<'a>(handlers: impl IntoIterator<Item = (&'a str, Type)>) -> Type {
    Type::Record(
        handlers
            .into_iter()
            .map(|(ability, ty)| (Key::Ability(Path::parse(ability)), ty))
            .collect(),
    )
}

/// `record`, a record type, with a value of type `ty` besides under
/// the key at a dotted path.
#[must_use]
pub fn keyed(record: Type, key: &str, ty: Type) -> Type {
    match record {
        Type::Record(mut fields) => {
            fields.push((Key::Ability(Path::parse(key)), ty));
            Type::Record(fields)
        }
        other => other,
    }
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

/// A method of a record type: what it needs of the record it is read
/// from, and the function reading it gives.
#[must_use]
pub fn method(needs: Type, function: Type) -> Type {
    Type::Method {
        needs: Box::new(needs),
        function: Box::new(function),
    }
}

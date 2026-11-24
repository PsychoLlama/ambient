//! Type constructors.

use crate::literal::Literal;
use crate::surface::{Key, Path, Pattern, Type, TypeMatch};

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

#[must_use]
pub fn bytes() -> Type {
    Type::Bytes
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

/// A function type with type parameters of its own, each a name and an
/// optional bound. `function` is a function type that names them, as
/// their bounds may; the names are not kept.
#[expect(
    clippy::needless_pass_by_value,
    reason = "a builder takes what it is given, as the others do"
)]
pub fn generic<'a>(
    params: impl IntoIterator<Item = (&'a str, Option<Type>)>,
    function: Type,
) -> Type {
    let (names, bounds): (Vec<&str>, Vec<Option<Type>>) = params.into_iter().unzip();
    let bind = |ty: &Type| {
        ty.rewrite(0, &|ty, depth| match ty {
            Type::Named(Path(segments), args) if args.is_empty() => {
                let [name] = segments.as_slice() else {
                    return None;
                };
                let position = names.iter().position(|param| param == name)?;
                Some(Type::Bound(depth + position))
            }
            _ => None,
        })
    };
    Type::Generic {
        bounds: bounds
            .iter()
            .map(|bound| bound.as_ref().map_or(Type::Unknown, &bind))
            .collect(),
        function: Box::new(bind(&function)),
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

/// A type match: `subject`, and arms that each give a type for what
/// their pattern matched. An arm's type names each binder of its
/// pattern by the name it binds. See "Type matches" in
/// `docs/spec/types.md`.
pub fn matching(subject: Type, arms: impl IntoIterator<Item = (Pattern, Type)>) -> Type {
    let arms = arms.into_iter().map(|(pattern, ty)| {
        let mut names = Vec::new();
        binders(&pattern, &mut names);
        let ty = named_binders(&ty, &names, 0);
        (pattern, ty)
    });
    Type::Match(Box::new(TypeMatch {
        subject,
        arms: arms.collect(),
    }))
}

/// The names `pattern` binds, in binding order.
fn binders<'a>(pattern: &'a Pattern, names: &mut Vec<&'a str>) {
    match pattern {
        Pattern::Wildcard | Pattern::Literal(_) | Pattern::Is(_) => {}
        Pattern::Bind(name, inner) => {
            names.push(name);
            binders(inner, names);
        }
        Pattern::Tuple(members) => members.iter().for_each(|member| binders(member, names)),
        Pattern::Record(fields) => fields.iter().for_each(|(_, inner)| binders(inner, names)),
    }
}

/// `ty` with each name of `names` it holds replaced by the binder it
/// names, inside `depth` binders of arms it is within.
fn named_binders(ty: &Type, names: &[&str], depth: usize) -> Type {
    ty.rewrite(0, &|ty, _| match ty {
        Type::Named(Path(segments), args) if args.is_empty() => {
            let [name] = segments.as_slice() else {
                return None;
            };
            // A later binder of the same name is the one named.
            let position = names.iter().rposition(|binder| binder == name)?;
            Some(Type::Matched(depth + position))
        }
        Type::Match(inner) => Some(Type::Match(Box::new(TypeMatch {
            subject: named_binders(&inner.subject, names, depth),
            arms: inner
                .arms
                .iter()
                .map(|(pattern, arm)| {
                    let pattern =
                        pattern.retyped(0, &|tested, _| Some(named_binders(tested, names, depth)));
                    let inside = depth + pattern.binders();
                    (pattern, named_binders(arm, names, inside))
                })
                .collect(),
        }))),
        _ => None,
    })
}

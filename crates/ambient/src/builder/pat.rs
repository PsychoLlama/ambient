//! Pattern constructors.

use crate::literal::Literal;
use crate::surface::{Pattern, Type};

/// Matches anything.
#[must_use]
pub fn any() -> Pattern {
    Pattern::Wildcard
}

/// Matches anything and binds it.
#[must_use]
pub fn bind(name: &str) -> Pattern {
    Pattern::Bind(name.to_owned(), Box::new(Pattern::Wildcard))
}

/// Matches what `pattern` matches and binds it.
#[must_use]
pub fn bind_as(name: &str, pattern: Pattern) -> Pattern {
    Pattern::Bind(name.to_owned(), Box::new(pattern))
}

#[must_use]
pub fn boolean(value: bool) -> Pattern {
    Pattern::Literal(Literal::Bool(value))
}

#[must_use]
pub fn int(value: i64) -> Pattern {
    Pattern::Literal(Literal::Int(value))
}

#[must_use]
pub fn string(value: &str) -> Pattern {
    Pattern::Literal(Literal::String(value.to_owned()))
}

/// Matches a value the type describes.
#[must_use]
pub fn is(ty: Type) -> Pattern {
    Pattern::Is(ty)
}

/// Matches a record with at least these fields.
pub fn record<'a>(fields: impl IntoIterator<Item = (&'a str, Pattern)>) -> Pattern {
    Pattern::Record(
        fields
            .into_iter()
            .map(|(name, pattern)| (name.to_owned(), pattern))
            .collect(),
    )
}

/// Matches a tuple of exactly this length.
pub fn tuple(members: impl IntoIterator<Item = Pattern>) -> Pattern {
    Pattern::Tuple(members.into_iter().collect())
}

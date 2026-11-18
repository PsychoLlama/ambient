//! Plain data: the expressions that are a value as they stand, which
//! is what an annotation says. See "Shape" in `spec/annotations.md`.

use super::{Expr, Key};
use crate::literal::Literal;
use crate::value::Value;

/// The value an expression of plain data is, if it is one: a literal,
/// or a list, a tuple, or a record of them.
#[must_use]
pub fn data<N>(expr: &Expr<N>) -> Option<Value> {
    let all = |members: &[Expr<N>]| members.iter().map(data).collect::<Option<Vec<_>>>();
    Some(match expr {
        Expr::Null => Value::Null,
        Expr::Literal(Literal::Bool(value)) => Value::Bool(*value),
        Expr::Literal(Literal::Int(value)) => Value::Int(*value),
        Expr::Literal(Literal::Float(bits)) => Value::Float(f64::from_bits(*bits)),
        Expr::Literal(Literal::String(value)) => Value::String(value.clone()),
        Expr::Literal(Literal::Bytes(value)) => Value::Bytes(value.clone()),
        Expr::Literal(Literal::Tag { package, tag }) => Value::Tag {
            package: package.clone(),
            tag: tag.clone(),
        },
        Expr::List(members) => Value::List(all(members)?),
        Expr::Tuple(members) => Value::Tuple(all(members)?),
        Expr::Record(fields) => Value::Record(
            fields
                .iter()
                .map(|(key, value)| match key {
                    Key::Field(name) => Some((name.clone(), data(value)?)),
                    Key::Ability(_) => None,
                })
                .collect::<Option<_>>()?,
        ),
        _ => return None,
    })
}

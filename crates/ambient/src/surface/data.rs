//! Plain data: the expressions that are a value as they stand, which
//! is what an annotation says. See "Shape" in `docs/spec/annotations.md`.

use super::{Expr, Key};
use crate::value::Value;

/// The value an expression of plain data is, if it is one: a literal,
/// a name `named` gives a value for, or a list, a tuple, or a record
/// of them. A name is how data holds a tag, or a reference to a
/// definition.
pub fn data<N>(expr: &Expr<N>, named: &dyn Fn(&N) -> Option<Value>) -> Option<Value> {
    let data = |expr: &Expr<N>| data(expr, named);
    let all = |members: &[Expr<N>]| members.iter().map(data).collect::<Option<Vec<_>>>();
    Some(match expr {
        Expr::Name(name) => named(name)?,
        Expr::Null => Value::Null,
        Expr::Literal(literal) => literal.value(),
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

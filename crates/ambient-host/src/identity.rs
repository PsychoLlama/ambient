//! Which type a function type is, as one name: what a function's
//! definition carries, and what a cast to a function type compares it
//! with. See "Types at run time" in `spec/backends.md`.
//!
//! Two function types have one name exactly when they are the same
//! type: a definition is named by its hash, and a union's members and a
//! record's fields are put in one order.

use std::fmt::Write;

use crate::core::{Key, Slot, Type};
use crate::hash::{Hash, Hasher};
use crate::literal::Literal;

/// The name of a type, or nothing if the type mentions a type
/// parameter: what it is then depends on where it is read.
pub(crate) fn name(ty: &Type, hash: &dyn Fn(Slot) -> Hash) -> Option<String> {
    let text = spell(ty, hash)?;
    Some(
        Hasher::new("type")
            .bytes(text.as_bytes())
            .finish()
            .to_string(),
    )
}

/// Each of `types` spelled, in the order given.
fn each(types: &[Type], hash: &dyn Fn(Slot) -> Hash) -> Option<Vec<String>> {
    types.iter().map(|ty| spell(ty, hash)).collect()
}

/// Text no other type spells. Whatever a program wrote is written with
/// its length first, so that it cannot be taken for punctuation.
fn spell(ty: &Type, hash: &dyn Fn(Slot) -> Hash) -> Option<String> {
    let counted = |kind: char, text: &str| format!("{kind}{}:{text}", text.len());
    Some(match ty {
        Type::Never => "never".to_owned(),
        Type::Unknown => "unknown".to_owned(),
        Type::Null => "null".to_owned(),
        Type::Bool => "bool".to_owned(),
        Type::Int => "int".to_owned(),
        Type::Float => "float".to_owned(),
        Type::String => "string".to_owned(),
        Type::Bytes => "bytes".to_owned(),
        Type::Brand(brand) => counted('h', &format!("{}\0{}", brand.package, brand.brand)),
        // A brand on a type is not there as a program runs.
        Type::Branded(ty, _) => return spell(ty, hash),
        Type::Tag => "tag".to_owned(),
        Type::Literal(literal) => match literal {
            Literal::Bool(value) => format!("={value}"),
            Literal::Int(value) => format!("={value}"),
            Literal::Float(bits) => format!("=f{bits}"),
            Literal::String(text) => counted('s', text),
            Literal::Bytes(_) => counted('b', &literal.to_string()),
            Literal::Tag { package, tag } => counted('t', &Literal::tag_text(package, tag)),
        },
        Type::Record(fields) => {
            let mut entries = Vec::new();
            for (key, ty) in fields {
                let key = match key {
                    Key::Field(name) => counted('f', name),
                    Key::Ability(slot) => format!("@{}", hash(*slot)),
                };
                entries.push(format!("{key}={}", spell(ty, hash)?));
            }
            entries.sort();
            format!("{{{}}}", entries.join(";"))
        }
        Type::Tuple(members) => format!("({})", each(members, hash)?.join(",")),
        Type::Union(members) => {
            let mut members = each(members, hash)?;
            members.sort();
            members.dedup();
            format!("<{}>", members.join("|"))
        }
        Type::List(member) => format!("[{}]", spell(member, hash)?),
        Type::Method { needs, function } => {
            format!("method[{}]{}", spell(needs, hash)?, spell(function, hash)?)
        }
        Type::Cell(member) => format!("cell[{}]", spell(member, hash)?),
        Type::Iterator(member) => format!("iter[{}]", spell(member, hash)?),
        Type::Function { params, ret, row } => {
            let mut text = format!("fn({})", each(params, hash)?.join(","));
            let _ = write!(text, "->{}", spell(ret, hash)?);
            let mut row: Vec<String> = row.iter().map(|slot| hash(*slot).to_string()).collect();
            row.sort();
            text + "!" + &row.join(",")
        }
        Type::Reference(slot, args) => {
            format!("#{}<{}>", hash(*slot), each(args, hash)?.join(","))
        }
        Type::Generic { bounds, function } => {
            format!(
                "all[{}]{}",
                each(bounds, hash)?.join(","),
                spell(function, hash)?
            )
        }
        Type::Bound(index) => format!("^{index}"),
        Type::Parameter(_) => return None,
    })
}

//! Values as types: the type a tree a comptime call gave stands for.
//! See "Splice positions" in `spec/comptime.md`.
//!
//! This reads what [`super::reflect`] writes, from the literal a
//! comptime result was spliced as.

use ambient_runtime::FAULT_PACKAGE;

use crate::core::{Expr, Key};
use crate::literal::Literal;
use crate::surface::{self, Reference};
use crate::symbol::{ModuleId, Symbol};
use crate::types::Ty;

/// The module of the library that says what a tree is.
pub(super) const MODULE: &str = "tree";

/// The type of a type's tree: what a parameter has that is handed a
/// type, and what a call gives that a type is derived from.
#[must_use]
pub fn tree() -> Symbol {
    ModuleId::new(FAULT_PACKAGE, MODULE).ty("Type")
}

/// The type a spliced `tree.Type` stands for, if it stands for one:
/// a brand and an ability are named in a tree by identity alone, which
/// does not say whose they are, so a tree with either stands for none.
#[must_use]
pub fn written(tree: &Expr) -> Option<Ty> {
    match tree {
        Expr::Literal(Literal::Tag { package, tag }) if package == FAULT_PACKAGE => {
            primitive(tag.strip_prefix(MODULE)?.strip_prefix('.')?)
        }
        Expr::Record(fields) => record(fields),
        _ => None,
    }
}

fn primitive(name: &str) -> Option<Ty> {
    Some(match name {
        "none" => Ty::Never,
        "any" => Ty::Unknown,
        "nil" => Ty::Null,
        "boolean" => Ty::Bool,
        "integer" => Ty::Int,
        "number" => Ty::Float,
        "text" => Ty::String,
        "binary" => Ty::Bytes,
        "tagged" => Ty::Tag,
        _ => return None,
    })
}

fn all(trees: &Expr) -> Option<Vec<Ty>> {
    let Expr::List(trees) = trees else {
        return None;
    };
    trees.iter().map(written).collect()
}

fn empty(list: &Expr) -> bool {
    matches!(list, Expr::List(members) if members.is_empty())
}

fn record(fields: &[(Key, Expr)]) -> Option<Ty> {
    let field = |name: &str| {
        let named = |(key, _): &&(Key, Expr)| matches!(key, Key::Field(field) if field == name);
        fields.iter().find(named).map(|(_, value)| value)
    };
    let boxed = |name: &str| field(name).and_then(written).map(Box::new);
    Some(match fields {
        [(Key::Field(name), holds)] => match (name.as_str(), holds) {
            ("literal", Expr::Literal(literal)) => Ty::Literal(literal.clone()),
            ("list", _) => Ty::List(boxed("list")?),
            ("mutable", _) => Ty::Cell(boxed("mutable")?),
            ("iterator", _) => Ty::Iterator(boxed("iterator")?),
            ("tuple", members) => Ty::Tuple(all(members)?),
            ("union", members) => Ty::Union(all(members)?),
            ("record", Expr::List(members)) => {
                Ty::Record(members.iter().map(member).collect::<Option<_>>()?)
            }
            _ => return None,
        },
        _ => Ty::Function {
            params: all(field("function")?)?,
            ret: boxed("returns")?,
            row: field("takes")
                .filter(|row| empty(row))
                .map(|_| Vec::new())?,
        },
    })
}

/// A field of a record type, from its `tree.Field`.
fn member(field: &Expr) -> Option<(surface::Key<Reference>, Ty)> {
    let Expr::Record(parts) = field else {
        return None;
    };
    match parts.as_slice() {
        [
            (Key::Field(holds), ty),
            (Key::Field(name), Expr::Literal(Literal::String(named))),
        ] if holds == "holds" && name == "name" => {
            Some((surface::Key::Field(named.clone()), written(ty)?))
        }
        _ => None,
    }
}

//! Values as types: the type a tree a comptime call gave stands for.
//! See "Splice positions" in `spec/comptime.md`.
//!
//! This reads what [`super::reflect`] writes, from the literal a
//! comptime result was spliced as.

use ambient_runtime::FAULT_PACKAGE;

use crate::core::{Expr, Key};
use crate::literal::{Brand, Literal};
use crate::surface::{self, Reference};
use crate::symbol::{ModuleId, Namespace, Symbol};
use crate::types::Ty;

/// The module of the library that says what a tree is.
pub(super) const MODULE: &str = "tree";

/// The type of a type's tree: what a parameter has that is handed a
/// type, and what a call gives that a type is derived from.
#[must_use]
pub fn tree() -> Symbol {
    ModuleId::new(FAULT_PACKAGE, MODULE).ty("Type")
}

/// The type of a reference: what a parameter has that is handed the
/// definition its argument names.
#[must_use]
pub fn reference() -> Symbol {
    ModuleId::new(FAULT_PACKAGE, MODULE).ty("Reference")
}

/// The type a spliced `tree.Type` stands for, if it stands for one.
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

/// The field `name` of a record that was spliced.
pub(super) fn field<'a>(fields: &'a [(Key, Expr)], name: &str) -> Option<&'a Expr> {
    let named = |(key, _): &&(Key, Expr)| matches!(key, Key::Field(field) if field == name);
    fields.iter().find(named).map(|(_, value)| value)
}

pub(super) fn text(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Literal(Literal::String(text)) => Some(text),
        _ => None,
    }
}

/// The type of an expression's tree: what a function gives for its
/// result to be spliced as code.
#[must_use]
pub fn code() -> Symbol {
    ModuleId::new(FAULT_PACKAGE, MODULE).ty("Expr")
}

/// The definition a spliced `tree.Reference` refers to, if it is of
/// one of the namespaces `kinds` name.
pub(super) fn definition(reference: &Expr, kinds: &[&str]) -> Option<Reference> {
    let Expr::Record(fields) = reference else {
        return None;
    };
    let text = |name: &str| field(fields, name).and_then(text);
    let Expr::List(path) = field(fields, "module")? else {
        return None;
    };
    let path = path.iter().map(text_of).collect::<Option<Vec<_>>>()?;
    let namespace = match text("kind")? {
        "type" => Namespace::Type,
        "value" => Namespace::Value,
        "ability" => Namespace::Ability,
        "key" => Namespace::Key,
        _ => return None,
    };
    if !kinds.contains(&text("kind")?) {
        return None;
    }
    let module = ModuleId::new(text("package")?, &path.join("."));
    Some(Reference::Definition(
        module.symbol(namespace, text("name")?),
    ))
}

fn text_of(expr: &Expr) -> Option<String> {
    text(expr).map(str::to_owned)
}

/// The ability a spliced `tree.Ability` is.
fn taken(ability: &Expr) -> Option<Reference> {
    let Expr::Record(fields) = ability else {
        return None;
    };
    definition(field(fields, "ability")?, &["ability"])
}

/// The brand a spliced `tree.Brand` is.
fn brand(brand: &Expr) -> Option<Ty> {
    let Expr::Record(fields) = brand else {
        return None;
    };
    let text = |name: &str| field(fields, name).and_then(text_of);
    Some(Ty::Brand(Brand {
        package: text("package")?,
        brand: text("identity")?,
    }))
}

pub(super) fn each<T>(list: &Expr, read: impl Fn(&Expr) -> Option<T>) -> Option<Vec<T>> {
    let Expr::List(members) = list else {
        return None;
    };
    members.iter().map(read).collect()
}

fn record(fields: &[(Key, Expr)]) -> Option<Ty> {
    let field = |name: &str| field(fields, name);
    let boxed = |name: &str| field(name).and_then(written).map(Box::new);
    Some(match fields {
        [(Key::Field(name), holds)] => match (name.as_str(), holds) {
            ("literal", Expr::Literal(literal)) => Ty::Literal(literal.clone()),
            ("list", _) => Ty::List(boxed("list")?),
            ("mutable", _) => Ty::Cell(boxed("mutable")?),
            ("iterator", _) => Ty::Iterator(boxed("iterator")?),
            ("tuple", members) => Ty::Tuple(all(members)?),
            ("union", members) => Ty::Union(all(members)?),
            ("named", named) => Ty::Named(definition(named, &["type"])?, Vec::new()),
            ("record", Expr::List(members)) => {
                Ty::Record(members.iter().map(member).collect::<Option<_>>()?)
            }
            _ => return None,
        },
        _ if field("brands").is_some() => {
            let brands = each(field("brands")?, brand)?;
            match (field("under")?, brands.as_slice()) {
                (Expr::Null, [handle]) => handle.clone(),
                (Expr::Null, _) => return None,
                (under, _) => Ty::Branded(Box::new(written(under)?), brands),
            }
        }
        _ => Ty::Function {
            params: all(field("function")?)?,
            ret: boxed("returns")?,
            row: each(field("takes")?, taken)?,
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

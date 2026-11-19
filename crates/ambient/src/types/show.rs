//! Types as error messages show them: as the text writes them, where
//! it has a way to. A type parameter and a method have no form of
//! their own, and a type nothing wrote out, as a tuple value's is, has
//! no names to show.

use std::fmt::Write as _;

use super::Ty;
use crate::surface::{Key, Members, Reference};

fn ability_name(ability: &Reference) -> &str {
    match ability {
        Reference::Definition(symbol) => &symbol.name,
        Reference::Local(_) | Reference::Parameter(_) => "?",
    }
}

fn show_all(types: &[Ty], separator: &str, out: &mut String) {
    for (index, ty) in types.iter().enumerate() {
        if index > 0 {
            out.push_str(separator);
        }
        show(ty, out);
    }
}

/// Members, each after the name it was written with if it has one.
fn show_members(members: &Members<Reference>, out: &mut String) {
    for (index, ty) in members.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        if let Some(name) = members.name(index) {
            let _ = write!(out, "{name}: ");
        }
        show(ty, out);
    }
}

/// A member of a union: a function type among others is in brackets,
/// since what it gives and takes would otherwise run on into them.
fn show_alternative(ty: &Ty, out: &mut String) {
    if matches!(ty, Ty::Function { .. } | Ty::Generic { .. }) {
        out.push('(');
        show(ty, out);
        out.push(')');
    } else {
        show(ty, out);
    }
}

/// A type as an error message shows it.
#[must_use]
pub fn shown(ty: &Ty) -> String {
    let mut out = String::new();
    show(ty, &mut out);
    out
}

fn show(ty: &Ty, out: &mut String) {
    match ty {
        Ty::Never => out.push_str("never"),
        Ty::Unknown => out.push_str("unknown"),
        Ty::Null => out.push_str("null"),
        Ty::Bool => out.push_str("bool"),
        Ty::Int => out.push_str("int"),
        Ty::Float => out.push_str("float"),
        Ty::String => out.push_str("string"),
        Ty::Bytes => out.push_str("bytes"),
        Ty::Tag => out.push_str("Tag"),
        Ty::Literal(literal) => {
            let _ = write!(out, "{literal}");
        }
        Ty::Record(fields) => {
            out.push('{');
            for (index, (key, ty)) in fields.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                let _ = match key {
                    Key::Field(name) => write!(out, "{name}: "),
                    Key::Ability(ability) => write!(out, "[{}]: ", ability_name(ability)),
                };
                show(ty, out);
            }
            out.push('}');
        }
        Ty::Tuple(members) => {
            out.push('(');
            show_members(members, out);
            out.push(')');
        }
        Ty::Cell(member) => {
            out.push_str("Cell<");
            show(member, out);
            out.push('>');
        }
        Ty::Brand(brand) => out.push_str(&brand.brand),
        Ty::Branded(ty, brands) => {
            show(ty, out);
            for brand in brands {
                out.push_str(" & ");
                show(brand, out);
            }
        }
        Ty::Iterator(member) => {
            out.push_str("Iterator<");
            show(member, out);
            out.push('>');
        }
        Ty::Method { needs, function } => {
            out.push_str("method(");
            show(needs, out);
            out.push_str(") ");
            show(function, out);
        }
        Ty::List(member) => {
            out.push_str("List<");
            show(member, out);
            out.push('>');
        }
        Ty::Function { params, ret, row } => {
            out.push_str("fn(");
            show_members(params, out);
            out.push_str("): ");
            show(ret, out);
            if !row.is_empty() {
                let names: Vec<&str> = row.iter().map(ability_name).collect();
                let _ = write!(out, " with {}", names.join(", "));
            }
        }
        Ty::Union(members) => {
            for (index, member) in members.iter().enumerate() {
                if index > 0 {
                    out.push_str(" | ");
                }
                show_alternative(member, out);
            }
        }
        Ty::Bound(index) => {
            let _ = write!(out, "G{index}");
        }
        Ty::Generic { bounds, function } => {
            out.push('<');
            show_all(bounds, ", ", out);
            out.push('>');
            show(function, out);
        }
        Ty::Named(Reference::Local(number) | Reference::Parameter(number), _) => {
            let _ = write!(out, "T{number}");
        }
        Ty::Named(Reference::Definition(symbol), args) => {
            out.push_str(&symbol.name);
            if !args.is_empty() {
                out.push('<');
                show_all(args, ", ", out);
                out.push('>');
            }
        }
    }
}

//! Types as error messages show them: as the text writes them. A type
//! parameter has no name once it is checked, so one is shown by its
//! number, and a type nothing wrote out, as a tuple value's is, has no
//! names to show.

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

/// What a function takes and gives, after `opening`: `fn`, or the key a
/// method is under. Type parameters come first, each with its bound if
/// it has one, and then `self` with what a method needs of it.
fn show_signature(opening: &str, receiver: Option<&Ty>, function: &Ty, out: &mut String) {
    out.push_str(opening);
    let function = match function {
        Ty::Generic { bounds, function } => {
            out.push('<');
            for (index, bound) in bounds.iter().enumerate() {
                let _ = write!(out, "{}G{index}", if index > 0 { ", " } else { "" });
                if !matches!(bound, Ty::Unknown) {
                    out.push_str(": ");
                    show(bound, out);
                }
            }
            out.push('>');
            function
        }
        function => function,
    };
    let Ty::Function { params, ret, row } = function else {
        // Nothing makes a method or a generic of anything else.
        return show(function, out);
    };
    out.push('(');
    if let Some(needs) = receiver {
        out.push_str("self: ");
        show(needs, out);
        if !params.is_empty() {
            out.push_str(", ");
        }
    }
    show_members(params, out);
    out.push_str("): ");
    // A function it gives is in brackets, or the row would read as
    // that one's.
    show_alternative(ret, out);
    if !row.is_empty() {
        let names: Vec<&str> = row.iter().map(ability_name).collect();
        let _ = write!(out, " with {}", names.join(", "));
    }
}

/// A member of a union, or what a function gives: a function type
/// there is in brackets, since what it gives and takes would otherwise
/// run on into what follows.
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
        Ty::Callable => out.push_str("a function"),
        Ty::Literal(literal) => {
            let _ = write!(out, "{literal}");
        }
        Ty::Record(fields) => {
            out.push('{');
            for (index, (key, ty)) in fields.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                let name = match key {
                    Key::Field(name) => name.clone(),
                    Key::Ability(ability) => format!("[{}]", ability_name(ability)),
                };
                // A method is written as its key and what it takes.
                if let Ty::Method { needs, function } = ty {
                    show_signature(&name, Some(needs), function, out);
                } else {
                    let _ = write!(out, "{name}: ");
                    show(ty, out);
                }
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
        // A method away from its key has no form of its own: it is shown
        // as a function that takes its receiver.
        Ty::Method { needs, function } => show_signature("fn", Some(needs), function, out),
        Ty::List(member) => {
            out.push_str("List<");
            show(member, out);
            out.push('>');
        }
        Ty::Function { .. } | Ty::Generic { .. } => show_signature("fn", None, ty, out),
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

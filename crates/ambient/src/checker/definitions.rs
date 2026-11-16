//! Definitions as trees: the function a `tree.Function` a comptime
//! call gave stands for, and the definitions of a `tree.Module`. See
//! "Splice positions" in `spec/comptime.md`.
//!
//! Each is read into what resolution leaves of one that was written,
//! so the engine checks it as it would that.

use super::code::Reader;
use super::reify::{Read, definition, each, field, fields, need, text, written};
use crate::core;
use crate::literal::Literal;
use crate::surface::{
    Ability, Definition, DefinitionKind, Function, Param, Reference, TypeDefinition,
};

/// The function a `tree.Function` a call gave stands for. Its
/// parameters are numbered as resolution numbers those of a function
/// that was written: first, and from nothing.
///
/// # Errors
///
/// Fails, saying why, if it stands for none.
pub fn defined(tree: &core::Expr) -> Read<Function<Reference>> {
    let fields = fields(tree, "a function")?;
    let mut reader = Reader::from(0);
    let params = params(need(fields, "params")?)?;
    for _ in &params {
        reader.bind();
    }
    Ok(Function {
        type_params: Vec::new(),
        params,
        ret: written(need(fields, "returns")?)?,
        row: each(need(fields, "takes")?, |taken| {
            definition(taken, &["ability"])
        })?,
        body: reader.expression(need(fields, "body")?)?,
    })
}

/// Parameters of the types a list of `tree.Type` stands for, named as
/// nothing a program writes is.
fn params(types: &core::Expr) -> Read<Vec<Param<Reference>>> {
    let param = |(at, ty)| Param {
        name: format!("__{at}"),
        ty,
    };
    Ok(each(types, written)?
        .into_iter()
        .enumerate()
        .map(param)
        .collect())
}

/// The definitions a `tree.Module` a call gave stands for, each as
/// resolution would leave one that was written. A tag is left a tag:
/// what it means holds where it is declared, which the engine knows.
///
/// # Errors
///
/// Fails, saying why, if one stands for nothing or has a name no
/// definition could.
pub fn definitions(tree: &core::Expr) -> Read<Vec<Definition<Reference>>> {
    each(tree, |entry| {
        let fields = fields(entry, "a definition")?;
        let name = text(need(fields, "name")?)?;
        let core::Expr::Literal(Literal::Bool(public)) = need(fields, "public")? else {
            return Err(format!("`{name}` does not say whether it is public"));
        };
        // A name is what tells apart an ability or a tag, with the path
        // of its module: one with a dot in it could be taken for
        // another module's.
        if !writable(name) {
            return Err(format!("`{name}` is no name a definition could have"));
        }
        Ok(Definition {
            name: name.to_owned(),
            public: *public,
            doc: None,
            kind: kind(fields).map_err(|why| format!("`{name}`: {why}"))?,
        })
    })
}

/// Whether a program could write `name` for a definition: it is an
/// identifier, and not one of those the reader makes up.
fn writable(name: &str) -> bool {
    let mut chars = name.chars();
    let starts = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_');
    starts && chars.all(|c| c.is_alphanumeric() || c == '_') && !name.starts_with("__")
}

/// What a `tree.Definition` with these fields is.
fn kind(fields: &[(core::Key, core::Expr)]) -> Read<DefinitionKind<Reference>> {
    let has = |name: &str| field(fields, name);
    Ok(if let Some(function) = has("function") {
        DefinitionKind::Function(defined(function)?)
    } else if let Some(takes) = has("ability") {
        DefinitionKind::Ability(Ability {
            type_params: Vec::new(),
            params: params(takes)?,
            ret: written(need(fields, "returns")?)?,
        })
    } else if has("tag").is_some() {
        DefinitionKind::Tag
    } else {
        DefinitionKind::Type(TypeDefinition {
            params: Vec::new(),
            ty: written(need(fields, "means")?)?,
        })
    })
}

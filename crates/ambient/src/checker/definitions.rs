//! Definitions as trees: the function a `tree.Function` a comptime
//! call gave stands for, and the definitions of a `tree.Module`. See
//! "Splice positions" in `spec/comptime.md`.
//!
//! Each is read into what resolution leaves of one that was written,
//! so the engine checks it as it would that.

use super::code::Reader;
use super::reify::{
    Home, Read, definition, each, field, fields, need, parameter_within, text, written,
};
use crate::core;
use crate::literal::Literal;
use crate::surface::{
    Ability, Definition, DefinitionKind, Function, Param, Reference, TypeDefinition, TypeParam,
};
use crate::symbol::ModuleId;
use crate::types::Ty;

/// The function a `tree.Function` a call gave stands for. Its
/// parameters are numbered as resolution numbers those of a function
/// that was written: first, and from nothing.
///
/// # Errors
///
/// Fails, saying why, if it stands for none.
pub fn defined(tree: &core::Expr) -> Read<Function<Reference>> {
    function(None, tree)
}

/// The function a `tree.Function` stands for, as a definition of
/// `home` if it is one of a module a call gave.
fn function(home: Home, tree: &core::Expr) -> Read<Function<Reference>> {
    let fields = fields(tree, "a function")?;
    let mut reader = Reader::new(home, 0);
    let params = params(home, need(fields, "params")?)?;
    for _ in &params {
        reader.bind();
    }
    let function = Function {
        type_params: type_params(home, need(fields, "generic")?)?,
        params,
        ret: written(home, need(fields, "returns")?)?,
        row: each(need(fields, "takes")?, |taken| {
            definition(home, taken, &["ability"])
        })?,
        body: reader.expression(need(fields, "body")?)?,
    };
    let count = function.type_params.len();
    let mut within = Ok(());
    let mut held = |name: &Reference| within = within.clone().and(parameter_within(count, name));
    function.header_names(&mut held);
    function.body.names(&mut held);
    within.map(|()| function)
}

/// Type parameters with the bounds a list of `tree.Type` or null
/// stands for, named as nothing a program writes is.
fn type_params(home: Home, bounds: &core::Expr) -> Read<Vec<TypeParam<Reference>>> {
    let bound = |bound: &core::Expr| match bound {
        core::Expr::Null => Ok(None),
        bound => written(home, bound).map(Some),
    };
    let named = |(at, bound)| TypeParam {
        name: format!("__T{at}"),
        bound,
    };
    Ok(each(bounds, bound)?
        .into_iter()
        .enumerate()
        .map(named)
        .collect())
}

/// A type that names only the first `count` type parameters.
fn within(home: Home, count: usize, tree: &core::Expr) -> Read<Ty> {
    let ty = written(home, tree)?;
    let mut within = Ok(());
    ty.names(&mut |name| within = within.clone().and(parameter_within(count, name)));
    within.map(|()| ty)
}

/// Parameters of the types a list of `tree.Type` stands for, named as
/// nothing a program writes is.
fn params(home: Home, types: &core::Expr) -> Read<Vec<Param<Reference>>> {
    let param = |(at, ty)| Param {
        name: format!("__{at}"),
        ty,
    };
    Ok(each(types, |ty| written(home, ty))?
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
/// definition could. What one names as beside it is a definition of
/// `home`, the module they are the content of.
pub fn definitions(tree: &core::Expr, home: &ModuleId) -> Read<Vec<Definition<Reference>>> {
    let home = Some(home);
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
            kind: kind(home, fields).map_err(|why| format!("`{name}`: {why}"))?,
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
fn kind(home: Home, fields: &[(core::Key, core::Expr)]) -> Read<DefinitionKind<Reference>> {
    let has = |name: &str| field(fields, name);
    Ok(if let Some(function) = has("function") {
        DefinitionKind::Function(self::function(home, function)?)
    } else if let Some(takes) = has("ability") {
        let type_params = type_params(home, need(fields, "generic")?)?;
        let count = type_params.len();
        let params = params(home, takes)?;
        let mut held = Ok(());
        for param in &params {
            let ty = &param.ty;
            ty.names(&mut |name| held = held.clone().and(parameter_within(count, name)));
        }
        held?;
        DefinitionKind::Ability(Ability {
            type_params,
            params,
            ret: within(home, count, need(fields, "returns")?)?,
        })
    } else if has("tag").is_some() {
        DefinitionKind::Tag
    } else {
        let core::Expr::Literal(Literal::Int(count)) = need(fields, "parameters")? else {
            return Err("a type does not say how many parameters it has".to_owned());
        };
        let count =
            usize::try_from(*count).map_err(|_| format!("a type has {count} parameters"))?;
        DefinitionKind::Type(TypeDefinition {
            params: (0..count).map(|at| format!("__T{at}")).collect(),
            ty: within(home, count, need(fields, "means")?)?,
        })
    })
}

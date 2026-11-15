//! What makes a tree well-formed, apart from its types.

use std::collections::BTreeSet;

use super::BuildError;
use crate::core::DEPTH;
use crate::surface::{Expr, Extern, ExternKind, Function, Key, Param, Path, Pattern, Target};

pub(super) fn check_name(name: &str) -> Result<(), BuildError> {
    let mut chars = name.chars();
    let starts = chars
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_');
    if starts && chars.all(|c| c.is_alphanumeric() || c == '_') {
        Ok(())
    } else {
        Err(BuildError::InvalidName(name.to_owned()))
    }
}

pub(super) fn check_path(path: &Path) -> Result<(), BuildError> {
    path.0.iter().try_for_each(|segment| check_name(segment))
}

pub(super) fn check_fields<'a>(names: impl Iterator<Item = &'a String>) -> Result<(), BuildError> {
    let mut seen = BTreeSet::new();
    for name in names {
        check_name(name)?;
        if !seen.insert(name) {
            return Err(BuildError::DuplicateField(name.clone()));
        }
    }
    Ok(())
}

/// The keys of a record or a record type: each once.
fn check_keys<'a>(keys: impl Iterator<Item = &'a Key>) -> Result<(), BuildError> {
    let mut seen = BTreeSet::new();
    for key in keys {
        match key {
            Key::Field(name) => check_name(name)?,
            Key::Ability(path) => check_path(path)?,
        }
        if !seen.insert(key) {
            return Err(match key {
                Key::Field(name) => BuildError::DuplicateField(name.clone()),
                Key::Ability(path) => BuildError::DuplicateHandler(path.clone()),
            });
        }
    }
    Ok(())
}

pub(super) fn check_type_params<'a>(
    names: impl Iterator<Item = &'a String>,
) -> Result<(), BuildError> {
    let mut seen = BTreeSet::new();
    for name in names {
        check_name(name)?;
        if !seen.insert(name) {
            return Err(BuildError::DuplicateTypeParameter(name.clone()));
        }
    }
    Ok(())
}

pub(super) fn check_function(name: &str, function: &Function) -> Result<(), BuildError> {
    check_type_params(function.type_params.iter().map(|param| &param.name))?;
    for bound in function.type_params.iter().filter_map(|p| p.bound.as_ref()) {
        check_type(bound)?;
    }
    check_params(name, &function.params)?;
    check_type(&function.ret)?;
    function.row.iter().try_for_each(check_path)?;
    check_expr(&function.body)
}

pub(super) fn check_extern(name: &str, native: &Extern) -> Result<(), BuildError> {
    match &native.kind {
        ExternKind::Function { params, ret } => {
            check_params(name, params)?;
            check_type(ret)
        }
        ExternKind::Handler { ability, row } => {
            check_path(ability)?;
            row.iter().try_for_each(check_path)
        }
    }
}

pub(super) fn check_target(target: &Target) -> Result<(), BuildError> {
    target.extends.iter().try_for_each(check_path)?;
    let mut bound = BTreeSet::new();
    for (ability, handler) in &target.bindings {
        check_path(ability)?;
        check_path(handler)?;
        if !bound.insert(ability) {
            return Err(BuildError::DuplicateHandler(ability.clone()));
        }
    }
    Ok(())
}

pub(super) fn check_params(name: &str, params: &[Param]) -> Result<(), BuildError> {
    let mut seen = BTreeSet::new();
    for param in params {
        check_name(&param.name)?;
        check_type(&param.ty)?;
        if !seen.insert(&param.name) {
            return Err(BuildError::DuplicateParameter {
                function: name.to_owned(),
                name: param.name.clone(),
            });
        }
    }
    Ok(())
}

pub(super) fn check_type(ty: &crate::surface::Type) -> Result<(), BuildError> {
    // A parameter of a generic function type is named only inside it.
    if !ty.closed() {
        return Err(BuildError::GenericType);
    }
    check_inside(ty)
}

/// [`check_type`], of a type that may be inside a generic function type.
fn check_inside(ty: &crate::surface::Type) -> Result<(), BuildError> {
    use crate::surface::Type;
    match ty {
        Type::Never
        | Type::Unknown
        | Type::Null
        | Type::Bool
        | Type::Int
        | Type::Float
        | Type::String
        | Type::Bytes
        | Type::Tag
        | Type::Bound(_) => Ok(()),
        Type::Literal(literal) => check_literal(literal),
        Type::Generic { bounds, function } => {
            if !matches!(**function, Type::Function { .. }) {
                return Err(BuildError::GenericType);
            }
            bounds.iter().try_for_each(check_inside)?;
            check_inside(function)
        }
        Type::Record(fields) => {
            check_keys(fields.iter().map(|(key, _)| key))?;
            fields.iter().try_for_each(|(_, ty)| check_inside(ty))
        }
        Type::Tuple(members) => {
            check_tuple(members.len())?;
            members.iter().try_for_each(check_inside)
        }
        Type::Union(members) => members.iter().try_for_each(check_inside),
        Type::List(member) | Type::Cell(member) | Type::Iterator(member) => check_inside(member),
        Type::Method { needs, function } => {
            check_inside(needs)?;
            check_inside(function)
        }
        Type::Function { params, ret, row } => {
            params.iter().try_for_each(check_inside)?;
            row.iter().try_for_each(check_path)?;
            check_inside(ret)
        }
        Type::Named(path, args) => {
            check_path(path)?;
            args.iter().try_for_each(check_inside)
        }
    }
}

/// A tuple groups values: none is nothing to group, which null says,
/// and one is that value.
fn check_tuple(members: usize) -> Result<(), BuildError> {
    if members < 2 {
        return Err(BuildError::ShortTuple);
    }
    Ok(())
}

pub(super) fn check_pattern<'a>(
    pattern: &'a Pattern,
    bound: &mut BTreeSet<&'a String>,
) -> Result<(), BuildError> {
    match pattern {
        Pattern::Wildcard => Ok(()),
        Pattern::Literal(literal) => check_literal(literal),
        Pattern::Record(fields) => {
            check_fields(fields.iter().map(|(name, _)| name))?;
            fields
                .iter()
                .try_for_each(|(_, pattern)| check_pattern(pattern, bound))
        }
        Pattern::Tuple(members) => {
            check_tuple(members.len())?;
            members
                .iter()
                .try_for_each(|member| check_pattern(member, bound))
        }
        Pattern::Is(ty) => check_type(ty),
        Pattern::Bind(name, inner) => {
            check_name(name)?;
            if !bound.insert(name) {
                return Err(BuildError::DuplicateBinder(name.clone()));
            }
            check_pattern(inner, bound)
        }
    }
}

/// Checks an expression and everything inside it, in the order written.
///
/// This is the one walk that does not recurse: it is what holds a tree
/// to the depth every other walk has room for.
pub(super) fn check_expr(expr: &Expr) -> Result<(), BuildError> {
    let mut pending = vec![(expr, 1)];
    while let Some((expr, depth)) = pending.pop() {
        if depth > DEPTH {
            return Err(BuildError::Nesting);
        }
        check_node(expr)?;
        let from = pending.len();
        expr.children(&mut |child| pending.push((child, depth + 1)));
        // The first child is to be taken first.
        pending[from..].reverse();
    }
    Ok(())
}

/// Checks what a node holds besides the expressions inside it.
/// A tag is a declaration: none is written where it is used.
fn check_literal(literal: &crate::literal::Literal) -> Result<(), BuildError> {
    match literal {
        crate::literal::Literal::Tag { tag, .. } => Err(BuildError::WrittenTag(tag.clone())),
        _ => Ok(()),
    }
}

fn check_node(expr: &Expr) -> Result<(), BuildError> {
    match expr {
        Expr::Literal(literal) => check_literal(literal),
        Expr::Name(path) | Expr::AbilityId(path) | Expr::Perform { ability: path, .. } => {
            check_path(path)
        }
        Expr::Instantiate { function, args } => {
            check_path(function)?;
            args.iter().try_for_each(check_type)
        }
        Expr::Record(fields) => check_keys(fields.iter().map(|(key, _)| key)),
        Expr::Field { key, .. } | Expr::Send { method: key, .. } => {
            check_keys(std::iter::once(key))
        }
        Expr::Let { name, .. } => check_name(name),
        Expr::Match { arms, .. } => arms
            .iter()
            .try_for_each(|arm| check_pattern(&arm.pattern, &mut BTreeSet::new())),
        Expr::At(..) => Err(BuildError::Mark),
        // An all gives the tuple of what its branches give.
        Expr::Tuple(members) | Expr::All(members) => check_tuple(members.len()),
        Expr::Cell { ty, .. } => ty.iter().try_for_each(check_type),
        Expr::Cast { ty, .. } => check_type(ty),
        Expr::Lambda {
            type_params,
            params,
            row,
            returns,
            ..
        } => {
            check_type_params(type_params.iter().map(|param| &param.name))?;
            for bound in type_params.iter().filter_map(|param| param.bound.as_ref()) {
                check_type(bound)?;
            }
            row.iter().try_for_each(check_path)?;
            returns.iter().try_for_each(check_type)?;
            let mut seen = BTreeSet::new();
            for (name, ty) in params {
                check_name(name)?;
                if !seen.insert(name) {
                    return Err(BuildError::DuplicateBinder(name.clone()));
                }
                if let Some(ty) = ty {
                    check_type(ty)?;
                }
            }
            Ok(())
        }
        Expr::Comptime(call) => {
            let named =
                matches!(&**call, Expr::Call { callee, .. } if matches!(**callee, Expr::Name(_)));
            if named {
                Ok(())
            } else {
                Err(BuildError::ComptimeNotCall)
            }
        }
        _ => Ok(()),
    }
}

//! What makes a tree well-formed, apart from its types.

use std::collections::BTreeSet;

use super::BuildError;
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
    use crate::surface::Type;
    match ty {
        Type::Never
        | Type::Unknown
        | Type::Null
        | Type::Bool
        | Type::Int
        | Type::Float
        | Type::String
        | Type::Literal(_) => Ok(()),
        Type::Record(fields) => {
            check_keys(fields.iter().map(|(key, _)| key))?;
            fields.iter().try_for_each(|(_, ty)| check_type(ty))
        }
        Type::Tuple(members) | Type::Union(members) => members.iter().try_for_each(check_type),
        Type::List(member) | Type::Cell(member) => check_type(member),
        Type::Function { params, ret, row } => {
            params.iter().try_for_each(check_type)?;
            row.iter().try_for_each(check_path)?;
            check_type(ret)
        }
        Type::Named(path, args) => {
            check_path(path)?;
            args.iter().try_for_each(check_type)
        }
    }
}

pub(super) fn check_pattern<'a>(
    pattern: &'a Pattern,
    bound: &mut BTreeSet<&'a String>,
) -> Result<(), BuildError> {
    match pattern {
        Pattern::Wildcard | Pattern::Literal(_) => Ok(()),
        Pattern::Record(fields) => {
            check_fields(fields.iter().map(|(name, _)| name))?;
            fields
                .iter()
                .try_for_each(|(_, pattern)| check_pattern(pattern, bound))
        }
        Pattern::Tuple(members) => members
            .iter()
            .try_for_each(|member| check_pattern(member, bound)),
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

pub(super) fn check_expr(expr: &Expr) -> Result<(), BuildError> {
    match expr {
        Expr::Null | Expr::Literal(_) => Ok(()),
        Expr::Name(path) => check_path(path),
        Expr::Instantiate { function, args } => {
            check_path(function)?;
            args.iter().try_for_each(check_type)
        }
        Expr::Call { callee, args } => {
            check_expr(callee)?;
            args.iter().try_for_each(check_expr)
        }
        Expr::Record(fields) => {
            check_keys(fields.iter().map(|(key, _)| key))?;
            fields.iter().try_for_each(|(_, value)| check_expr(value))
        }
        Expr::Tuple(members) | Expr::List(members) => members.iter().try_for_each(check_expr),
        Expr::Field { target, key } => {
            check_keys(std::iter::once(key))?;
            check_expr(target)
        }
        Expr::Let { name, value, body } => {
            check_name(name)?;
            check_expr(value)?;
            check_expr(body)
        }
        Expr::Binary { left, right, .. } | Expr::Merge { left, right } => {
            check_expr(left)?;
            check_expr(right)
        }
        Expr::Match { subject, arms } => {
            check_expr(subject)?;
            arms.iter().try_for_each(|arm| {
                check_pattern(&arm.pattern, &mut BTreeSet::new())?;
                check_expr(&arm.body)
            })
        }
        Expr::Perform { ability, args } => {
            check_path(ability)?;
            args.iter().try_for_each(check_expr)
        }
        Expr::Provide { bundle, body, .. } => {
            check_expr(bundle)?;
            check_expr(body)
        }
        Expr::Observe { observer, body } => {
            check_expr(observer)?;
            check_expr(body)
        }
        Expr::AbilityId(ability) => check_path(ability),
        Expr::All(branches) => branches.iter().try_for_each(check_expr),
        Expr::Cell { ty, args, .. } => {
            ty.iter().try_for_each(check_type)?;
            args.iter().try_for_each(check_expr)
        }
        Expr::Lambda { params, row, body } => {
            row.iter().try_for_each(check_path)?;
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
            check_expr(body)
        }
        Expr::Comptime(call) => {
            let named =
                matches!(&**call, Expr::Call { callee, .. } if matches!(**callee, Expr::Name(_)));
            if !named {
                return Err(BuildError::ComptimeNotCall);
            }
            check_expr(call)
        }
        Expr::Cast {
            value,
            ty,
            otherwise,
        } => {
            check_expr(value)?;
            check_type(ty)?;
            check_expr(otherwise)
        }
    }
}

//! What makes a tree well-formed, apart from its types.

use std::collections::BTreeSet;

use super::BuildError;
use crate::surface::{Expr, Function, Path, Pattern};

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
    let mut seen = BTreeSet::new();
    for param in &function.params {
        check_name(&param.name)?;
        check_type(&param.ty)?;
        if !seen.insert(&param.name) {
            return Err(BuildError::DuplicateParameter {
                function: name.to_owned(),
                name: param.name.clone(),
            });
        }
    }
    check_type(&function.ret)?;
    check_expr(&function.body)
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
            check_fields(fields.iter().map(|(name, _)| name))?;
            fields.iter().try_for_each(|(_, ty)| check_type(ty))
        }
        Type::Tuple(members) | Type::Union(members) => members.iter().try_for_each(check_type),
        Type::List(member) => check_type(member),
        Type::Function { params, ret } => {
            params.iter().try_for_each(check_type)?;
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
            check_fields(fields.iter().map(|(name, _)| name))?;
            fields.iter().try_for_each(|(_, value)| check_expr(value))
        }
        Expr::Tuple(members) | Expr::List(members) => members.iter().try_for_each(check_expr),
        Expr::Field { target, name } => {
            check_name(name)?;
            check_expr(target)
        }
        Expr::Let { name, value, body } => {
            check_name(name)?;
            check_expr(value)?;
            check_expr(body)
        }
        Expr::Binary { left, right, .. } => {
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
        Expr::Lambda { params, body } => {
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

//! Structural target matching and rigid-parameter substitution: the
//! pattern/binding walks a conditional impl's target shape flows through —
//! [`match_target`] binds a pattern's [`Type::Param`]s against a concrete
//! type, [`substitute_rigid_params`] rewrites bound params back in, and
//! [`invert_named`] is the diagnostics-side dual over a generic struct's
//! `Named` placeholders. Split from `constraints.rs` (per-file line
//! budgets); the semantics live with the solver there.

use std::collections::HashMap;
use std::sync::Arc;

use crate::types::Type;

/// Substitute [`Type::Param`]s by name — the inverse direction of
/// [`match_target`]'s binding: once a conditional impl's parameters are
/// assigned (`T -> Money`), rewrite a type that *mentions* those parameters
/// (an inner bound's trait argument, `From<T>`) to its concrete form.
/// Unassigned params pass through unchanged.
pub(crate) fn substitute_rigid_params(ty: &Type, subst: &HashMap<Arc<str>, Type>) -> Type {
    match ty {
        Type::Param(name) => subst.get(name).cloned().unwrap_or_else(|| ty.clone()),
        Type::Named(n) => Type::Named(
            n.map_args(
                n.args
                    .iter()
                    .map(|a| substitute_rigid_params(a, subst))
                    .collect(),
            ),
        ),
        Type::Nominal(nom) => {
            Type::Nominal(nom.map_inner(substitute_rigid_params(&nom.inner, subst)))
        }
        Type::Tuple(elems) => Type::Tuple(
            elems
                .iter()
                .map(|e| substitute_rigid_params(e, subst))
                .collect(),
        ),
        Type::Record(rec) => Type::Record(crate::types::RecordType::new(
            rec.fields
                .iter()
                .map(|(n, t)| (Arc::clone(n), substitute_rigid_params(t, subst)))
                .collect(),
        )),
        Type::Function(f) => Type::function_with_abilities(
            f.params
                .iter()
                .map(|p| substitute_rigid_params(p, subst))
                .collect(),
            substitute_rigid_params(&f.ret, subst),
            f.abilities.clone(),
        ),
        // A projection over an assigned parameter (`T::Error` with
        // `T -> Money`) keeps the projection form; whoever holds the impl's
        // associated bindings eliminates it. Substituting only the base
        // keeps this walk a pure param rewrite.
        Type::Projection(p) => p.with_base(substitute_rigid_params(&p.base, subst)),
        _ => ty.clone(),
    }
}

/// One-directional structural match of a conditional impl's target shape
/// (`pattern`, carrying the impl's [`Type::Param`]s) against a concrete
/// `concrete` type, binding each param to the type it lines up with.
///
/// This is *matching*, not full unification: only `pattern`'s params bind,
/// and a param that recurs must line up with an equal type each time
/// (`Pair<T>` against `Pair<Money>` binds `T = Money` consistently). A
/// structural mismatch — a different head uuid, arity, or field set — is a
/// clean `false`, which the caller reports as an unsatisfied bound.
pub(crate) fn match_target(
    pattern: &Type,
    concrete: &Type,
    subst: &mut HashMap<Arc<str>, Type>,
) -> bool {
    match (pattern, concrete) {
        (Type::Param(name), _) => {
            if let Some(existing) = subst.get(name) {
                existing == concrete
            } else {
                subst.insert(Arc::clone(name), concrete.clone());
                true
            }
        }
        (Type::Named(p), Type::Named(c)) => {
            let head_ok = match (p.uuid, c.uuid) {
                (Some(a), Some(b)) => a == b,
                _ => p.name == c.name,
            };
            head_ok
                && p.args.len() == c.args.len()
                && p.args
                    .iter()
                    .zip(&c.args)
                    .all(|(a, b)| match_target(a, b, subst))
        }
        (Type::Nominal(p), Type::Nominal(c)) => {
            p.uuid == c.uuid && match_target(&p.inner, &c.inner, subst)
        }
        (Type::Record(p), Type::Record(c)) => {
            p.fields.len() == c.fields.len()
                && p.fields
                    .iter()
                    .zip(&c.fields)
                    .all(|((pn, pt), (cn, ct))| pn == cn && match_target(pt, ct, subst))
        }
        (Type::Tuple(p), Type::Tuple(c)) => {
            p.len() == c.len() && p.iter().zip(c).all(|(a, b)| match_target(a, b, subst))
        }
        (Type::Function(p), Type::Function(c)) => {
            p.params.len() == c.params.len()
                && p.params
                    .iter()
                    .zip(&c.params)
                    .all(|(a, b)| match_target(a, b, subst))
                && match_target(&p.ret, &c.ret, subst)
        }
        _ => pattern == concrete,
    }
}

/// Invert [`substitute_named`] for diagnostics: match a generic struct's
/// declared body `pattern` (its fields written as bare `Named(param)`
/// placeholders) against a concrete instantiation, binding each declared
/// `param` to the type it lines up with. The dual of [`match_target`], which
/// binds [`Type::Param`]s; here the binders are the placeholder `Named`s a
/// fielded generic struct's body carries. A structural mismatch is a clean
/// `false`, leaving the caller to fall back to the bare-head rendering.
///
/// [`substitute_named`]: super::check::subst::substitute_named
pub(super) fn invert_named(
    pattern: &Type,
    concrete: &Type,
    params: &[Arc<str>],
    subst: &mut HashMap<Arc<str>, Type>,
) -> bool {
    // A bare placeholder naming one of the struct's parameters binds it.
    if let Type::Named(p) = pattern
        && p.args.is_empty()
        && p.uuid.is_none()
        && params.iter().any(|q| q == &p.name)
    {
        return if let Some(existing) = subst.get(&p.name) {
            existing == concrete
        } else {
            subst.insert(Arc::clone(&p.name), concrete.clone());
            true
        };
    }
    match (pattern, concrete) {
        (Type::Named(p), Type::Named(c)) => {
            let head_ok = match (p.uuid, c.uuid) {
                (Some(a), Some(b)) => a == b,
                _ => p.name == c.name,
            };
            head_ok
                && p.args.len() == c.args.len()
                && p.args
                    .iter()
                    .zip(&c.args)
                    .all(|(a, b)| invert_named(a, b, params, subst))
        }
        (Type::Nominal(p), Type::Nominal(c)) => {
            p.uuid == c.uuid && invert_named(&p.inner, &c.inner, params, subst)
        }
        (Type::Record(p), Type::Record(c)) => {
            p.fields.len() == c.fields.len()
                && p.fields
                    .iter()
                    .zip(&c.fields)
                    .all(|((pn, pt), (cn, ct))| pn == cn && invert_named(pt, ct, params, subst))
        }
        (Type::Tuple(p), Type::Tuple(c)) => {
            p.len() == c.len()
                && p.iter()
                    .zip(c)
                    .all(|(a, b)| invert_named(a, b, params, subst))
        }
        (Type::Function(p), Type::Function(c)) => {
            p.params.len() == c.params.len()
                && p.params
                    .iter()
                    .zip(&c.params)
                    .all(|(a, b)| invert_named(a, b, params, subst))
                && invert_named(&p.ret, &c.ret, params, subst)
        }
        _ => pattern == concrete,
    }
}

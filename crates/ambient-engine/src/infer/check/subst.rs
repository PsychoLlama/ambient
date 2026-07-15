//! Type-parameter substitution used when instantiating generic declarations.
//!
//! Two closely related walks over a [`Type`], differing only in what a
//! parameter name maps to:
//!
//! - [`substitute_type_params`] maps a name to a fresh inference variable id
//!   (`Named("T")` → `Var(?0)`), the shape enum constructors and inherent-impl
//!   method schemes need.
//! - [`substitute_named`] maps a name to an arbitrary [`Type`], the shape a
//!   *fielded* generic struct application needs: construction substitutes fresh
//!   variables, an annotated application (`Box<Number, String>`) substitutes
//!   the written arguments.
//!
//! Both recurse through the same type shapes — including `Type::Nominal`, so a
//! generic struct's record body has its parameters rewritten while its nominal
//! identity rides along — so the two can never drift on which positions a
//! parameter reaches.

use std::collections::HashMap;
use std::sync::Arc;

use crate::types::{Type, TypeVarId};

/// Substitute type parameters in a type with type variables.
pub(in crate::infer) fn substitute_type_params(
    ty: &Type,
    type_var_map: &HashMap<Arc<str>, TypeVarId>,
) -> Type {
    match ty {
        Type::Named(named) => {
            // Check if this is a type parameter reference
            if named.args.is_empty()
                && let Some(&var_id) = type_var_map.get(&named.name)
            {
                return Type::var(var_id);
            }
            // Otherwise, recursively substitute in type arguments, preserving
            // any nominal identity (a declared enum's uuid).
            Type::Named(
                named.map_args(
                    named
                        .args
                        .iter()
                        .map(|arg| substitute_type_params(arg, type_var_map))
                        .collect(),
                ),
            )
        }
        Type::Function(f) => Type::function_with_abilities(
            f.params
                .iter()
                .map(|p| substitute_type_params(p, type_var_map))
                .collect(),
            substitute_type_params(&f.ret, type_var_map),
            f.abilities.clone(),
        ),
        Type::Tuple(elems) => Type::Tuple(
            elems
                .iter()
                .map(|e| substitute_type_params(e, type_var_map))
                .collect(),
        ),
        Type::Record(rec) => Type::Record(crate::types::RecordType::new(
            rec.fields
                .iter()
                .map(|(n, t)| (Arc::clone(n), substitute_type_params(t, type_var_map)))
                .collect(),
        )),
        // A nominal body (a generic struct's substituted record) carries its
        // parameters inside `inner`; recurse so they map to their variables
        // while the identity rides along.
        Type::Nominal(nom) => {
            Type::Nominal(nom.map_inner(substitute_type_params(&nom.inner, type_var_map)))
        }
        // A projection's base may name a parameter (`T::Error`); the
        // projection itself is not a parameter reference.
        Type::Projection(p) => p.with_base(substitute_type_params(&p.base, type_var_map)),
        // Primitives and other types pass through unchanged
        _ => ty.clone(),
    }
}

/// Substitute a generic struct's named type parameters with concrete types.
///
/// The single mechanism for applying a generic *fielded* struct: given the
/// struct's declared body (fields written as bare `Named(param)` placeholders)
/// and a map from parameter name to the type to substitute in, produce the
/// instantiated body. Construction passes fresh type variables (arguments
/// inferred); an annotated applied form (`Box<Number, String>`) passes the
/// written arguments. Mirrors [`substitute_type_params`] but maps a name to an
/// arbitrary [`Type`] rather than a fresh variable id, so the two never drift.
#[must_use]
pub(in crate::infer) fn substitute_named(ty: &Type, map: &HashMap<Arc<str>, Type>) -> Type {
    match ty {
        Type::Named(named) => {
            if named.args.is_empty()
                && let Some(replacement) = map.get(&named.name)
            {
                return replacement.clone();
            }
            Type::Named(
                named.map_args(
                    named
                        .args
                        .iter()
                        .map(|arg| substitute_named(arg, map))
                        .collect(),
                ),
            )
        }
        Type::Function(f) => Type::function_with_abilities(
            f.params.iter().map(|p| substitute_named(p, map)).collect(),
            substitute_named(&f.ret, map),
            f.abilities.clone(),
        ),
        Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| substitute_named(e, map)).collect()),
        Type::Record(rec) => Type::Record(crate::types::RecordType::new(
            rec.fields
                .iter()
                .map(|(n, t)| (Arc::clone(n), substitute_named(t, map)))
                .collect(),
        )),
        Type::Nominal(nom) => Type::Nominal(nom.map_inner(substitute_named(&nom.inner, map))),
        Type::Projection(p) => p.with_base(substitute_named(&p.base, map)),
        // Primitives and other types pass through unchanged
        _ => ty.clone(),
    }
}

/// Rewrite `Self::X` spellings in a trait-method signature into
/// [`Type::Projection`]s over `Self`, for each `X` the trait declares as an
/// associated type. The parser has no projection syntax — a qualified name
/// in type position lowers to a `::`-joined `Named` head — so this is where
/// the checker gives the spelling its meaning. An `X` the trait does not
/// declare stays a bare `Named` (reported by the declaration validation).
pub(in crate::infer) fn project_self_assoc(
    ty: &Type,
    trait_uuid: uuid::Uuid,
    assoc_names: &[Arc<str>],
) -> Type {
    match ty {
        Type::Named(named) => {
            if named.args.is_empty()
                && let Some(assoc) = named.name.strip_prefix("Self::")
                && let Some(assoc) = assoc_names.iter().find(|n| n.as_ref() == assoc)
            {
                return Type::Projection(crate::types::ProjectionType {
                    base: Box::new(Type::Named(crate::types::NamedType::simple(Arc::from(
                        "Self",
                    )))),
                    trait_uuid,
                    assoc: Arc::clone(assoc),
                });
            }
            Type::Named(
                named.map_args(
                    named
                        .args
                        .iter()
                        .map(|arg| project_self_assoc(arg, trait_uuid, assoc_names))
                        .collect(),
                ),
            )
        }
        Type::Function(f) => Type::function_with_abilities(
            f.params
                .iter()
                .map(|p| project_self_assoc(p, trait_uuid, assoc_names))
                .collect(),
            project_self_assoc(&f.ret, trait_uuid, assoc_names),
            f.abilities.clone(),
        ),
        Type::Tuple(elems) => Type::Tuple(
            elems
                .iter()
                .map(|e| project_self_assoc(e, trait_uuid, assoc_names))
                .collect(),
        ),
        Type::Record(rec) => Type::Record(crate::types::RecordType::new(
            rec.fields
                .iter()
                .map(|(n, t)| {
                    (
                        Arc::clone(n),
                        project_self_assoc(t, trait_uuid, assoc_names),
                    )
                })
                .collect(),
        )),
        Type::Nominal(nom) => {
            Type::Nominal(nom.map_inner(project_self_assoc(&nom.inner, trait_uuid, assoc_names)))
        }
        _ => ty.clone(),
    }
}

/// Eliminate the projections a dispatching impl's associated bindings
/// resolve: `Self::Error` (or, post-`substitute_self`, `T::Error`) becomes
/// the bound type when `map` carries the trait's binding for that name.
/// Projections of *other* traits (a different uuid) and unmapped names pass
/// through — they belong to some enclosing scope.
pub(in crate::infer) fn substitute_assoc(
    ty: &Type,
    trait_uuid: uuid::Uuid,
    map: &HashMap<Arc<str>, Type>,
) -> Type {
    match ty {
        Type::Projection(p) => {
            if p.trait_uuid == trait_uuid
                && let Some(replacement) = map.get(&p.assoc)
            {
                return replacement.clone();
            }
            p.with_base(substitute_assoc(&p.base, trait_uuid, map))
        }
        Type::Named(named) => Type::Named(
            named.map_args(
                named
                    .args
                    .iter()
                    .map(|arg| substitute_assoc(arg, trait_uuid, map))
                    .collect(),
            ),
        ),
        Type::Function(f) => Type::function_with_abilities(
            f.params
                .iter()
                .map(|p| substitute_assoc(p, trait_uuid, map))
                .collect(),
            substitute_assoc(&f.ret, trait_uuid, map),
            f.abilities.clone(),
        ),
        Type::Tuple(elems) => Type::Tuple(
            elems
                .iter()
                .map(|e| substitute_assoc(e, trait_uuid, map))
                .collect(),
        ),
        Type::Record(rec) => Type::Record(crate::types::RecordType::new(
            rec.fields
                .iter()
                .map(|(n, t)| (Arc::clone(n), substitute_assoc(t, trait_uuid, map)))
                .collect(),
        )),
        Type::Nominal(nom) => {
            Type::Nominal(nom.map_inner(substitute_assoc(&nom.inner, trait_uuid, map)))
        }
        _ => ty.clone(),
    }
}

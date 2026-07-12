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
        // Primitives and other types pass through unchanged
        _ => ty.clone(),
    }
}

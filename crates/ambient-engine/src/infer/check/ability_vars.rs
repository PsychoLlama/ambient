//! Effect-polymorphism plumbing for a declaration's generic parameters.
//!
//! A generics list mixes ordinary type parameters (`T`) with **ability (row)
//! variables** (`E!`). This module splits the two — allocating a fresh type
//! variable per `T` and a fresh [`AbilityVarId`] per `E!` — and resolves a
//! declared `with` clause against the ability variables so `with E` becomes
//! the row's polymorphic tail. The type checker keys off the resulting
//! [`crate::infer::env::Scheme`]; effects are erased before compilation, so
//! none of this reaches the compiler or the content hash.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{QualifiedName, TypeParam};
use crate::infer::Infer;
use crate::infer::error::{BoxedTypeErrorExt, TypeError, TypeErrorKind};
use crate::types::{AbilityId, AbilitySet, AbilityVarId, TypeVarId};

/// A declaration's generic parameters, split into type variables and ability
/// (row) variables with fresh ids allocated for each.
pub(super) struct GenericScope {
    /// Type-parameter name → fresh quantified type variable.
    pub(super) type_var_map: HashMap<Arc<str>, TypeVarId>,
    /// The quantified type variables, in declaration order.
    pub(super) quantified_type_vars: Vec<TypeVarId>,
    /// Ability-variable name → fresh quantified ability variable. The scope a
    /// signature and body resolve `with E` positions against.
    pub(super) ability_var_map: HashMap<Arc<str>, AbilityVarId>,
    /// The quantified ability variables, in declaration order.
    pub(super) ability_vars: Vec<AbilityVarId>,
}

impl GenericScope {
    /// Whether this declaration has no quantified variables at all (so its
    /// scheme is monomorphic).
    pub(super) fn is_empty(&self) -> bool {
        self.quantified_type_vars.is_empty() && self.ability_vars.is_empty()
    }
}

/// Partition `type_params` into type variables and ability (row) variables,
/// allocating a fresh id for each. Ability variables (`E!`) carry no trait
/// bounds — lowering rejects `E!: Bound` — so they never enter
/// [`crate::ast::dict_params`] and perturb no dictionary machinery.
pub(super) fn generic_scope(infer: &mut Infer, type_params: &[TypeParam]) -> GenericScope {
    let mut type_var_map = HashMap::new();
    let mut quantified_type_vars = Vec::new();
    let mut ability_var_map = HashMap::new();
    let mut ability_vars = Vec::new();
    for tp in type_params {
        if tp.is_ability {
            let id = infer.r#gen.fresh_ability_id();
            ability_var_map.insert(Arc::clone(&tp.name), id);
            ability_vars.push(id);
        } else {
            // Quantified ids come from the shared generator so they can never
            // collide with inference variables allocated elsewhere.
            let id = infer.r#gen.fresh_id();
            type_var_map.insert(Arc::clone(&tp.name), id);
            quantified_type_vars.push(id);
        }
    }
    GenericScope {
        type_var_map,
        quantified_type_vars,
        ability_var_map,
        ability_vars,
    }
}

/// Build just the ability-variable scope (`E!` name → fresh [`AbilityVarId`])
/// for a declaration's generics.
///
/// A function/method *body* installs its own fresh row variables (distinct
/// from the scheme's) but treats the ordinary type parameters as *rigid*
/// names, not fresh type variables — so it needs only the ability half of
/// [`generic_scope`]. Calling `generic_scope` there would mint throwaway type
/// variables for every `T`; this allocates ability ids alone.
pub(super) fn ability_var_scope(
    infer: &mut Infer,
    type_params: &[TypeParam],
) -> HashMap<Arc<str>, AbilityVarId> {
    let mut ability_var_map = HashMap::new();
    for tp in type_params {
        if tp.is_ability {
            let id = infer.r#gen.fresh_ability_id();
            ability_var_map.insert(Arc::clone(&tp.name), id);
        }
    }
    ability_var_map
}

/// Resolve a declared `with` clause into an [`AbilitySet`], keying each bare
/// name against the declaration's ability variables first.
///
/// A bare name naming an ability variable is the row's polymorphic tail (it
/// shadows any in-scope ability of the same name); every other name resolves
/// to a concrete ability id under the namespace policy. A row has a single
/// tail, so a second distinct variable in one row is an error. Unknown
/// concrete names report through `pending_errors` — a typo must not quietly
/// declare the function pure.
pub(in crate::infer) fn resolve_declared_with(
    infer: &mut Infer,
    abilities: &[QualifiedName],
    ability_var_map: &HashMap<Arc<str>, AbilityVarId>,
    fn_name: &str,
) -> AbilitySet {
    let mut ids: Vec<AbilityId> = Vec::with_capacity(abilities.len());
    let mut tail: Option<(Arc<str>, AbilityVarId)> = None;
    for qn in abilities {
        if qn.path.is_empty()
            && let Some(&var) = ability_var_map.get(&qn.name)
        {
            match &tail {
                Some((first, existing)) if *existing != var => {
                    infer.pending_errors.push(Box::new(TypeError::new(
                        TypeErrorKind::MultipleRowVariables {
                            first: Arc::clone(first),
                            second: Arc::clone(&qn.name),
                        },
                        (0, 0),
                    )));
                }
                _ => tail = Some((Arc::clone(&qn.name), var)),
            }
            continue;
        }
        match infer.resolve_ability_or_set(qn, (0, 0)) {
            Ok(set) => ids.extend(set.concrete_abilities().iter().copied()),
            Err(e) => infer
                .pending_errors
                .push(e.with_context(format!("in `with` clause of function `{fn_name}`"))),
        }
    }
    match tail {
        // `row` collapses to a bare `Var` when there are no concrete ids.
        Some((_, var)) => AbilitySet::row(ids, var),
        None => AbilitySet::from_abilities(ids),
    }
}

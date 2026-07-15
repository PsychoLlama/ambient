//! Applying the substitution to types and ability sets, including the
//! `MaskedSubst` view used to apply under a `Forall` binder.

use crate::infer::Infer;
use crate::types::{AbilitySet, AbilityVarId, FunctionType, RecordType, Type, TypeVarId};

impl Infer {
    /// Apply substitution to a type.
    #[must_use]
    pub fn apply(&self, ty: &Type) -> Type {
        self.apply_impl(ty, &mut Vec::new())
    }

    /// Apply ability substitution to an ability set.
    ///
    /// Resolves substitution chains fully (`a → b → {Stdio}` yields
    /// `{Stdio}`, not `b`) — enforcement and effect propagation depend on
    /// reaching the concrete set at the end of a chain. Cycles (from
    /// recursive bindings) stop resolving and return the set as-is.
    #[must_use]
    pub fn apply_abilities(&self, abilities: &AbilitySet) -> AbilitySet {
        self.apply_abilities_impl(abilities, &mut Vec::new())
    }

    fn apply_abilities_impl(
        &self,
        abilities: &AbilitySet,
        visiting: &mut Vec<AbilityVarId>,
    ) -> AbilitySet {
        self.subst_view().apply_abilities(abilities, visiting)
    }

    pub(crate) fn apply_impl(&self, ty: &Type, seen: &mut Vec<TypeVarId>) -> Type {
        self.subst_view().apply(ty, seen)
    }

    /// The full substitution as a [`MaskedSubst`] with nothing masked.
    fn subst_view(&self) -> MaskedSubst<'_> {
        MaskedSubst {
            types: &self.subst,
            abilities: &self.ability_subst,
            masked_types: &[],
            masked_abilities: &[],
        }
    }
}

/// A view of the substitution with a binder's quantified variables masked
/// out, for applying under `Forall` without cloning the inference state.
struct MaskedSubst<'a> {
    types: &'a std::collections::HashMap<TypeVarId, Type>,
    abilities: &'a std::collections::HashMap<AbilityVarId, AbilitySet>,
    masked_types: &'a [TypeVarId],
    masked_abilities: &'a [AbilityVarId],
}

impl MaskedSubst<'_> {
    fn lookup(&self, id: TypeVarId) -> Option<&Type> {
        if self.masked_types.contains(&id) {
            None
        } else {
            self.types.get(&id)
        }
    }

    fn lookup_ability(&self, id: AbilityVarId) -> Option<&AbilitySet> {
        if self.masked_abilities.contains(&id) {
            None
        } else {
            self.abilities.get(&id)
        }
    }

    fn apply(&self, ty: &Type, seen: &mut Vec<TypeVarId>) -> Type {
        match ty {
            Type::Var(id) => {
                if seen.contains(id) {
                    return ty.clone(); // Cycle, stop
                }
                if let Some(bound) = self.lookup(*id) {
                    seen.push(*id);
                    let result = self.apply(bound, seen);
                    seen.pop();
                    result
                } else {
                    ty.clone()
                }
            }
            Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| self.apply(e, seen)).collect()),
            Type::Record(r) => Type::Record(RecordType::new(
                r.fields
                    .iter()
                    .map(|(n, t)| (n.clone(), self.apply(t, seen)))
                    .collect(),
            )),
            Type::Function(f) => Type::Function(FunctionType::with_abilities(
                f.params.iter().map(|p| self.apply(p, seen)).collect(),
                self.apply(&f.ret, seen),
                self.apply_abilities(&f.abilities, &mut Vec::new()),
            )),
            Type::Named(n) => {
                Type::Named(n.map_args(n.args.iter().map(|a| self.apply(a, seen)).collect()))
            }
            Type::Nominal(n) => Type::Nominal(n.map_inner(self.apply(&n.inner, seen))),
            // A projection's base is a rigid atom (`Param`) or the stored
            // `Named("Self")`; applying recurses for uniformity, the
            // projection itself is never substituted here.
            Type::Projection(p) => p.with_base(self.apply(&p.base, seen)),
            Type::Handler(h) => Type::Handler(crate::types::HandlerType::new(
                h.ability,
                self.apply(&h.answer, seen),
            )),
            Type::Forall(f) => {
                // A nested binder masks its own variables in addition.
                let mut masked_types = self.masked_types.to_vec();
                masked_types.extend_from_slice(&f.vars);
                let mut masked_abilities = self.masked_abilities.to_vec();
                masked_abilities.extend_from_slice(&f.ability_vars);
                let inner = MaskedSubst {
                    types: self.types,
                    abilities: self.abilities,
                    masked_types: &masked_types,
                    masked_abilities: &masked_abilities,
                };
                Type::Forall(crate::types::ForallType::with_abilities(
                    f.vars.clone(),
                    f.ability_vars.clone(),
                    inner.apply(&f.body, seen),
                ))
            }
            _ => ty.clone(),
        }
    }

    fn apply_abilities(
        &self,
        abilities: &AbilitySet,
        visiting: &mut Vec<AbilityVarId>,
    ) -> AbilitySet {
        match abilities {
            AbilitySet::Empty | AbilitySet::Concrete(_) | AbilitySet::Unresolved(_) => {
                abilities.clone()
            }
            AbilitySet::Var(id) => match self.lookup_ability(*id) {
                Some(bound) if !visiting.contains(id) => {
                    visiting.push(*id);
                    let resolved = self.apply_abilities(bound, visiting);
                    visiting.pop();
                    resolved
                }
                _ => abilities.clone(),
            },
            AbilitySet::Row { concrete, tail } => match self.lookup_ability(*tail) {
                Some(tail_set) if !visiting.contains(tail) => {
                    visiting.push(*tail);
                    let resolved_tail = self.apply_abilities(tail_set, visiting);
                    visiting.pop();
                    AbilitySet::from_abilities(concrete.iter().copied()).union(&resolved_tail)
                }
                _ => abilities.clone(),
            },
        }
    }
}

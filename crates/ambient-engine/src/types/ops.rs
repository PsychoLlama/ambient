//! Type algorithms: substitution, free-variable collection, and
//! concreteness checks.

use std::collections::HashMap;

use super::{
    AbilitySet, AbilityValueType, AbilityVarId, ForallType, FunctionType, HandlerType, RecordType,
    Type, TypeVarId,
};

impl Type {
    /// Check if this type is a concrete (non-variable) type.
    #[must_use]
    pub fn is_concrete(&self) -> bool {
        match self {
            Self::Var(_) | Self::Hole => false,
            Self::Tuple(elems) => elems.iter().all(Type::is_concrete),
            Self::Record(rec) => rec.fields.iter().all(|(_, t)| t.is_concrete()),
            Self::Function(f) => {
                f.params.iter().all(Type::is_concrete)
                    && f.ret.is_concrete()
                    && f.abilities.ability_var().is_none()
            }
            Self::Named(n) => n.args.iter().all(Type::is_concrete),
            Self::Nominal(n) => n.inner.is_concrete(),
            Self::Forall(f) => f.body.is_concrete(),
            Self::AbilityValue(av) => av.result.is_concrete() && av.ability.ability_var().is_none(),
            Self::Handler(h) => h.answer.is_concrete(),
            // All other types are concrete by default
            _ => true,
        }
    }

    /// Collect all free type variables in this type.
    #[must_use]
    pub fn free_vars(&self) -> Vec<TypeVarId> {
        let mut vars = Vec::new();
        self.collect_free_vars(&mut vars);
        vars.sort_unstable();
        vars.dedup();
        vars
    }

    fn collect_free_vars(&self, vars: &mut Vec<TypeVarId>) {
        match self {
            Self::Var(id) => vars.push(*id),
            Self::Tuple(elems) => {
                for elem in elems {
                    elem.collect_free_vars(vars);
                }
            }
            Self::Record(rec) => {
                for (_, t) in &rec.fields {
                    t.collect_free_vars(vars);
                }
            }
            Self::Function(f) => {
                for p in &f.params {
                    p.collect_free_vars(vars);
                }
                f.ret.collect_free_vars(vars);
            }
            Self::Named(n) => {
                for arg in &n.args {
                    arg.collect_free_vars(vars);
                }
            }
            Self::Nominal(n) => n.inner.collect_free_vars(vars),
            Self::AbilityValue(av) => av.result.collect_free_vars(vars),
            Self::Handler(h) => h.answer.collect_free_vars(vars),
            Self::Forall(f) => {
                // Bound variables are not free
                let mut body_vars = Vec::new();
                f.body.collect_free_vars(&mut body_vars);
                for var in body_vars {
                    if !f.vars.contains(&var) {
                        vars.push(var);
                    }
                }
            }
            _ => {}
        }
    }

    /// Collect all free ability variables in this type.
    #[must_use]
    pub fn free_ability_vars(&self) -> Vec<AbilityVarId> {
        let mut vars = Vec::new();
        self.collect_free_ability_vars(&mut vars);
        vars.sort_unstable();
        vars.dedup();
        vars
    }

    fn collect_free_ability_vars(&self, vars: &mut Vec<AbilityVarId>) {
        match self {
            Self::Function(f) => {
                for p in &f.params {
                    p.collect_free_ability_vars(vars);
                }
                f.ret.collect_free_ability_vars(vars);
                vars.extend(f.abilities.free_ability_vars());
            }
            Self::Tuple(elems) => {
                for elem in elems {
                    elem.collect_free_ability_vars(vars);
                }
            }
            Self::Record(rec) => {
                for (_, t) in &rec.fields {
                    t.collect_free_ability_vars(vars);
                }
            }
            Self::Named(n) => {
                for arg in &n.args {
                    arg.collect_free_ability_vars(vars);
                }
            }
            Self::Nominal(n) => n.inner.collect_free_ability_vars(vars),
            Self::AbilityValue(av) => {
                av.result.collect_free_ability_vars(vars);
                vars.extend(av.ability.free_ability_vars());
            }
            Self::Handler(h) => h.answer.collect_free_ability_vars(vars),
            Self::Forall(f) => {
                let mut body_vars = Vec::new();
                f.body.collect_free_ability_vars(&mut body_vars);
                for var in body_vars {
                    if !f.ability_vars.contains(&var) {
                        vars.push(var);
                    }
                }
            }
            _ => {}
        }
    }

    /// Substitute type variables with other types.
    #[must_use]
    pub fn substitute(&self, subst: &HashMap<TypeVarId, Type>) -> Type {
        self.substitute_all(subst, &HashMap::new())
    }

    /// Substitute both type variables and ability variables.
    #[must_use]
    pub fn substitute_all(
        &self,
        type_subst: &HashMap<TypeVarId, Type>,
        ability_subst: &HashMap<AbilityVarId, AbilitySet>,
    ) -> Type {
        match self {
            Self::Var(id) => type_subst.get(id).cloned().unwrap_or_else(|| self.clone()),
            Self::Tuple(elems) => Self::Tuple(
                elems
                    .iter()
                    .map(|e| e.substitute_all(type_subst, ability_subst))
                    .collect(),
            ),
            Self::Record(rec) => Self::Record(RecordType::new(
                rec.fields
                    .iter()
                    .map(|(n, t)| (n.clone(), t.substitute_all(type_subst, ability_subst)))
                    .collect(),
            )),
            Self::Function(f) => {
                let new_abilities = substitute_ability_set(&f.abilities, ability_subst);
                Self::Function(FunctionType::with_abilities(
                    f.params
                        .iter()
                        .map(|p| p.substitute_all(type_subst, ability_subst))
                        .collect(),
                    f.ret.substitute_all(type_subst, ability_subst),
                    new_abilities,
                ))
            }
            Self::Named(n) => Self::Named(
                n.map_args(
                    n.args
                        .iter()
                        .map(|a| a.substitute_all(type_subst, ability_subst))
                        .collect(),
                ),
            ),
            Self::Nominal(n) => {
                Self::Nominal(n.map_inner(n.inner.substitute_all(type_subst, ability_subst)))
            }
            Self::AbilityValue(av) => {
                let new_ability = substitute_ability_set(&av.ability, ability_subst);
                Self::AbilityValue(AbilityValueType::new(
                    av.result.substitute_all(type_subst, ability_subst),
                    new_ability,
                ))
            }
            Self::Handler(h) => Self::Handler(HandlerType::new(
                h.ability,
                h.answer.substitute_all(type_subst, ability_subst),
            )),
            Self::Forall(f) => {
                // Don't substitute bound variables
                let mut new_type_subst = type_subst.clone();
                for var in &f.vars {
                    new_type_subst.remove(var);
                }
                let mut new_ability_subst = ability_subst.clone();
                for var in &f.ability_vars {
                    new_ability_subst.remove(var);
                }
                Self::Forall(ForallType::with_abilities(
                    f.vars.clone(),
                    f.ability_vars.clone(),
                    f.body.substitute_all(&new_type_subst, &new_ability_subst),
                ))
            }
            _ => self.clone(),
        }
    }
}

/// Substitute ability variables in an ability set.
fn substitute_ability_set(
    ability_set: &AbilitySet,
    subst: &HashMap<AbilityVarId, AbilitySet>,
) -> AbilitySet {
    match ability_set {
        AbilitySet::Empty | AbilitySet::Concrete(_) | AbilitySet::Unresolved(_) => {
            ability_set.clone()
        }
        AbilitySet::Var(id) => subst
            .get(id)
            .cloned()
            .unwrap_or_else(|| ability_set.clone()),
        AbilitySet::Row { concrete, tail } => {
            if let Some(tail_set) = subst.get(tail) {
                // Merge concrete with the substituted tail
                AbilitySet::from_abilities(concrete.iter().copied()).union(tail_set)
            } else {
                ability_set.clone()
            }
        }
    }
}

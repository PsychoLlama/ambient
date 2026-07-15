//! Type algorithms: substitution, free-variable collection, and
//! concreteness checks.

use std::collections::HashMap;

use super::{
    AbilitySet, AbilityVarId, ForallType, FunctionType, HandlerType, RecordType, Type, TypeVarId,
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
            Self::Handler(h) => h.answer.is_concrete(),
            // All other types are concrete by default
            _ => true,
        }
    }

    /// Whether this type mentions a rigid [`Type::Param`] anywhere — i.e.
    /// its meaning depends on the enclosing item's type parameters.
    #[must_use]
    pub fn mentions_param(&self) -> bool {
        match self {
            Self::Param(_) => true,
            Self::Tuple(elems) => elems.iter().any(Type::mentions_param),
            Self::Record(rec) => rec.fields.iter().any(|(_, t)| t.mentions_param()),
            Self::Function(f) => {
                f.params.iter().any(Type::mentions_param) || f.ret.mentions_param()
            }
            Self::Named(n) => n.args.iter().any(Type::mentions_param),
            Self::Nominal(n) => n.inner.mentions_param(),
            Self::Projection(p) => p.base.mentions_param(),
            Self::Forall(f) => f.body.mentions_param(),
            Self::Handler(h) => h.answer.mentions_param(),
            _ => false,
        }
    }

    /// Display-only: recursively replace every function's ability set with its
    /// [`AbilitySet::closed`] form, so an unconstrained effect row (a
    /// generalized-but-uninstantiated `E!`) doesn't surface as `with E23!` in
    /// hover, completions, or item signatures. Recurses through composite types
    /// so a nested function type (a lambda-valued local, a function parameter)
    /// is normalized too. NOT used by unification, hashing, or diagnostics —
    /// those keep the raw row, where the tail variable is meaningful.
    #[must_use]
    pub fn close_rows_for_display(&self) -> Type {
        match self {
            Self::Tuple(elems) => {
                Self::Tuple(elems.iter().map(Type::close_rows_for_display).collect())
            }
            Self::Record(rec) => Self::Record(RecordType::new(
                rec.fields
                    .iter()
                    .map(|(n, t)| (n.clone(), t.close_rows_for_display()))
                    .collect(),
            )),
            Self::Function(f) => Self::Function(FunctionType::with_abilities(
                f.params.iter().map(Type::close_rows_for_display).collect(),
                f.ret.close_rows_for_display(),
                f.abilities.closed(),
            )),
            Self::Named(n) => {
                Self::Named(n.map_args(n.args.iter().map(Type::close_rows_for_display).collect()))
            }
            Self::Nominal(n) => Self::Nominal(n.map_inner(n.inner.close_rows_for_display())),
            Self::Handler(h) => Self::Handler(HandlerType::new(
                h.ability,
                h.answer.close_rows_for_display(),
            )),
            Self::Forall(f) => Self::Forall(ForallType::with_abilities(
                f.vars.clone(),
                f.ability_vars.clone(),
                f.body.close_rows_for_display(),
            )),
            _ => self.clone(),
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
            Self::Projection(p) => p.base.collect_free_vars(vars),
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
            Self::Projection(p) => p.with_base(p.base.substitute_all(type_subst, ability_subst)),
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

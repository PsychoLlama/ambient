//! Type unification for Hindley-Milner type inference.
//!
//! This module implements the unification algorithm which finds a most general
//! unifier (MGU) for two types. Unification works by:
//!
//! 1. Decomposing compound types (tuples, records, functions) and unifying
//!    their components recursively.
//! 2. Binding type variables to concrete types (with occurs check to prevent
//!    infinite types).
//! 3. Checking that primitive types match exactly.

use super::{Infer, InferResult, TypeErrorKind, type_error};
use crate::types::{AbilitySet, AbilityVarId, FunctionType, RecordType, Type, TypeVarId};

impl Infer {
    /// Unify two types and update the substitution.
    ///
    /// # Errors
    ///
    /// Returns a `TypeError` if the types cannot be unified.
    #[allow(clippy::too_many_lines)]
    pub fn unify(&mut self, t1: &Type, t2: &Type, span: (u32, u32)) -> InferResult<()> {
        let t1 = self.apply(t1);
        let t2 = self.apply(t2);

        match (&t1, &t2) {
            // Same primitive types
            // Primitive types and error types
            (Type::Unit, Type::Unit)
            | (Type::Bool, Type::Bool)
            | (Type::Number, Type::Number)
            | (Type::String, Type::String)
            | (Type::Bytes, Type::Bytes)
            | (Type::Never, Type::Never)
            | (Type::Error, _)
            | (_, Type::Error) => Ok(()),

            // Type variables
            (Type::Var(id1), Type::Var(id2)) if id1 == id2 => Ok(()),
            (Type::Var(id), ty) | (ty, Type::Var(id)) => {
                // Occurs check
                if self.occurs(*id, ty) {
                    return Err(type_error(
                        TypeErrorKind::InfiniteType {
                            var: *id,
                            ty: ty.clone(),
                        },
                        span,
                    ));
                }
                self.subst.insert(*id, ty.clone());
                Ok(())
            }

            // Tuples
            (Type::Tuple(elems1), Type::Tuple(elems2)) => {
                if elems1.len() != elems2.len() {
                    return Err(type_error(
                        TypeErrorKind::TypeMismatch {
                            expected: t1.clone(),
                            actual: t2.clone(),
                        },
                        span,
                    ));
                }
                for (e1, e2) in elems1.iter().zip(elems2.iter()) {
                    self.unify(e1, e2, span)?;
                }
                Ok(())
            }

            // Records
            (Type::Record(r1), Type::Record(r2)) => {
                if r1.fields.len() != r2.fields.len() {
                    return Err(type_error(
                        TypeErrorKind::TypeMismatch {
                            expected: t1.clone(),
                            actual: t2.clone(),
                        },
                        span,
                    ));
                }
                for ((n1, ty1), (n2, ty2)) in r1.fields.iter().zip(r2.fields.iter()) {
                    if n1 != n2 {
                        return Err(type_error(
                            TypeErrorKind::TypeMismatch {
                                expected: t1.clone(),
                                actual: t2.clone(),
                            },
                            span,
                        ));
                    }
                    self.unify(ty1, ty2, span)?;
                }
                Ok(())
            }

            // Functions
            (Type::Function(f1), Type::Function(f2)) => {
                if f1.params.len() != f2.params.len() {
                    return Err(type_error(
                        TypeErrorKind::TypeMismatch {
                            expected: t1.clone(),
                            actual: t2.clone(),
                        },
                        span,
                    ));
                }
                for (p1, p2) in f1.params.iter().zip(f2.params.iter()) {
                    self.unify(p1, p2, span)?;
                }
                self.unify(&f1.ret, &f2.ret, span)?;
                // Unify ability requirements (Milestone 8)
                self.unify_abilities(&f1.abilities, &f2.abilities, span)
            }

            // AbilityValue types (Milestone 8)
            (Type::AbilityValue(av1), Type::AbilityValue(av2)) => {
                self.unify(&av1.result, &av2.result, span)?;
                self.unify_abilities(&av1.ability, &av2.ability, span)
            }

            // Named types. The head name and arity must match. Nominal
            // identities (declared enums) must also agree, but a `None`
            // identity is a wildcard: it lets a payload self-reference or an
            // as-yet-unresolved annotation (both carry `None`) unify with the
            // resolved, uuid-carrying form. Two *distinct* enums never unify —
            // either their names differ, or both carry `Some` uuids that
            // differ.
            (Type::Named(n1), Type::Named(n2)) => {
                let identity_conflict = matches!(
                    (n1.uuid, n2.uuid),
                    (Some(u1), Some(u2)) if u1 != u2
                );
                if n1.name != n2.name || n1.args.len() != n2.args.len() || identity_conflict {
                    return Err(type_error(
                        TypeErrorKind::TypeMismatch {
                            expected: t1.clone(),
                            actual: t2.clone(),
                        },
                        span,
                    ));
                }
                for (a1, a2) in n1.args.iter().zip(n2.args.iter()) {
                    self.unify(a1, a2, span)?;
                }
                Ok(())
            }

            // Nominal types
            (Type::Nominal(n1), Type::Nominal(n2)) => {
                if n1.uuid != n2.uuid {
                    return Err(type_error(
                        TypeErrorKind::TypeMismatch {
                            expected: t1.clone(),
                            actual: t2.clone(),
                        },
                        span,
                    ));
                }
                self.unify(&n1.inner, &n2.inner, span)
            }

            // Mismatch
            _ => Err(type_error(
                TypeErrorKind::TypeMismatch {
                    expected: t1.clone(),
                    actual: t2.clone(),
                },
                span,
            )),
        }
    }

    /// Check if a type variable occurs in a type (after applying substitution).
    pub(crate) fn occurs(&self, var: TypeVarId, ty: &Type) -> bool {
        let ty = self.apply(ty);
        match ty {
            Type::Var(id) => id == var,
            Type::Tuple(elems) => elems.iter().any(|e| self.occurs(var, e)),
            Type::Record(r) => r.fields.iter().any(|(_, t)| self.occurs(var, t)),
            Type::Function(f) => {
                f.params.iter().any(|p| self.occurs(var, p)) || self.occurs(var, &f.ret)
            }
            Type::Named(n) => n.args.iter().any(|a| self.occurs(var, a)),
            Type::Nominal(n) => self.occurs(var, &n.inner),
            Type::AbilityValue(av) => self.occurs(var, &av.result),
            _ => false,
        }
    }

    /// Unify two ability sets.
    ///
    /// # Errors
    ///
    /// Returns a `TypeError` if the ability sets cannot be unified.
    #[allow(clippy::too_many_lines)]
    pub fn unify_abilities(
        &mut self,
        a1: &AbilitySet,
        a2: &AbilitySet,
        span: (u32, u32),
    ) -> InferResult<()> {
        let a1 = self.apply_abilities(a1);
        let a2 = self.apply_abilities(a2);

        match (&a1, &a2) {
            // Unresolved ability names are eliminated by resolve_holes before
            // unification; reaching here means an annotation bypassed
            // resolution. Report a mismatch rather than guessing.
            (AbilitySet::Unresolved(_), _) | (_, AbilitySet::Unresolved(_)) => Err(type_error(
                TypeErrorKind::AbilityMismatch {
                    expected: a1.clone(),
                    actual: a2.clone(),
                },
                span,
            )),

            // Both empty - trivially equal
            (AbilitySet::Empty, AbilitySet::Empty) => Ok(()),

            // Both concrete - must be equal
            (AbilitySet::Concrete(c1), AbilitySet::Concrete(c2)) => {
                if c1 == c2 {
                    Ok(())
                } else {
                    Err(type_error(
                        TypeErrorKind::AbilityMismatch {
                            expected: a1.clone(),
                            actual: a2.clone(),
                        },
                        span,
                    ))
                }
            }

            // Same variable - trivially equal
            (AbilitySet::Var(id1), AbilitySet::Var(id2)) if id1 == id2 => Ok(()),

            // Variable with anything - bind the variable
            (AbilitySet::Var(id), other) | (other, AbilitySet::Var(id)) => {
                // Occurs check for ability variables
                if self.ability_occurs(*id, other) {
                    return Err(type_error(
                        TypeErrorKind::InfiniteAbility {
                            var: *id,
                            abilities: other.clone(),
                        },
                        span,
                    ));
                }
                self.ability_subst.insert(*id, other.clone());
                Ok(())
            }

            // Empty with concrete - concrete must be empty
            (AbilitySet::Empty, AbilitySet::Concrete(c))
            | (AbilitySet::Concrete(c), AbilitySet::Empty) => {
                if c.is_empty() {
                    Ok(())
                } else {
                    Err(type_error(
                        TypeErrorKind::AbilityMismatch {
                            expected: a1.clone(),
                            actual: a2.clone(),
                        },
                        span,
                    ))
                }
            }

            // Row with something - need to unify carefully
            (
                AbilitySet::Row {
                    concrete: c1,
                    tail: t1,
                },
                AbilitySet::Row {
                    concrete: c2,
                    tail: t2,
                },
            ) => {
                // Same tail - concrete parts must match
                if t1 == t2 {
                    if c1 == c2 {
                        Ok(())
                    } else {
                        Err(type_error(
                            TypeErrorKind::AbilityMismatch {
                                expected: a1.clone(),
                                actual: a2.clone(),
                            },
                            span,
                        ))
                    }
                } else {
                    // Different tails: standard row unification under set
                    // semantics. Each tail absorbs the abilities the other
                    // side has and it lacks, and both share a fresh tail:
                    //   {c1 | t1} ~ {c2 | t2}
                    //   t1 := {c2 \ c1 | t3},  t2 := {c1 \ c2 | t3}
                    // Both sides then resolve to {c1 ∪ c2 | t3} — the most
                    // general unifier (rows are idempotent sets, so the
                    // previous "bind both tails to the full union" also
                    // forced each tail to contain its own side's concrete
                    // abilities, over-widening every later use of the tail).
                    let fresh_tail = self.r#gen.fresh_ability_id();
                    let only_in_c2: Vec<_> =
                        c2.iter().filter(|a| !c1.contains(a)).copied().collect();
                    let only_in_c1: Vec<_> =
                        c1.iter().filter(|a| !c2.contains(a)).copied().collect();
                    self.ability_subst
                        .insert(*t1, AbilitySet::row(only_in_c2, fresh_tail));
                    self.ability_subst
                        .insert(*t2, AbilitySet::row(only_in_c1, fresh_tail));
                    Ok(())
                }
            }

            // Row with concrete - check that concrete is a subset
            (
                AbilitySet::Row {
                    concrete: row_concrete,
                    tail,
                },
                AbilitySet::Concrete(c),
            )
            | (
                AbilitySet::Concrete(c),
                AbilitySet::Row {
                    concrete: row_concrete,
                    tail,
                },
            ) => {
                // Check that all row_concrete abilities are in c
                for ability in row_concrete {
                    if !c.contains(ability) {
                        return Err(type_error(
                            TypeErrorKind::AbilityMismatch {
                                expected: a1.clone(),
                                actual: a2.clone(),
                            },
                            span,
                        ));
                    }
                }
                // Bind the tail to the remaining abilities
                let remaining: Vec<_> = c
                    .iter()
                    .filter(|a| !row_concrete.contains(a))
                    .copied()
                    .collect();
                let remaining_set = AbilitySet::from_abilities(remaining);
                self.ability_subst.insert(*tail, remaining_set);
                Ok(())
            }

            // Row with empty - row must be empty
            (AbilitySet::Row { concrete, tail }, AbilitySet::Empty)
            | (AbilitySet::Empty, AbilitySet::Row { concrete, tail }) => {
                if concrete.is_empty() {
                    self.ability_subst.insert(*tail, AbilitySet::Empty);
                    Ok(())
                } else {
                    Err(type_error(
                        TypeErrorKind::AbilityMismatch {
                            expected: a1.clone(),
                            actual: a2.clone(),
                        },
                        span,
                    ))
                }
            }
        }
    }

    /// Check if an ability variable occurs in an ability set.
    pub(crate) fn ability_occurs(&self, var: AbilityVarId, abilities: &AbilitySet) -> bool {
        let abilities = self.apply_abilities(abilities);
        match &abilities {
            AbilitySet::Empty | AbilitySet::Concrete(_) | AbilitySet::Unresolved(_) => false,
            AbilitySet::Var(id) => *id == var,
            AbilitySet::Row { tail, .. } => *tail == var,
        }
    }

    /// Apply substitution to a type.
    #[must_use]
    pub fn apply(&self, ty: &Type) -> Type {
        self.apply_impl(ty, &mut Vec::new())
    }

    /// Apply ability substitution to an ability set.
    ///
    /// Resolves substitution chains fully (`a → b → {Console}` yields
    /// `{Console}`, not `b`) — enforcement and effect propagation depend on
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
            Type::Nominal(n) => Type::Nominal(crate::types::NominalType::new(
                n.uuid,
                self.apply(&n.inner, seen),
                n.name.clone(),
            )),
            Type::AbilityValue(av) => Type::AbilityValue(crate::types::AbilityValueType::new(
                self.apply(&av.result, seen),
                self.apply_abilities(&av.ability, &mut Vec::new()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infer::Scheme;
    use crate::infer::env::TypeEnv;
    use crate::types::{AbilityId, AbilitySet};

    /// A distinct, recognizable AbilityId for tests.
    fn aid(n: u8) -> AbilityId {
        AbilityId::from_bytes([n; 32])
    }

    fn span() -> (u32, u32) {
        (0, 0)
    }

    #[test]
    fn test_unify_primitives() {
        let mut infer = Infer::new();
        assert!(infer.unify(&Type::Number, &Type::Number, span()).is_ok());
        assert!(infer.unify(&Type::String, &Type::String, span()).is_ok());
        assert!(infer.unify(&Type::Bool, &Type::Bool, span()).is_ok());
        assert!(infer.unify(&Type::Unit, &Type::Unit, span()).is_ok());
    }

    #[test]
    fn test_unify_mismatch() {
        let mut infer = Infer::new();
        assert!(infer.unify(&Type::Number, &Type::String, span()).is_err());
        assert!(infer.unify(&Type::Bool, &Type::Number, span()).is_err());
    }

    #[test]
    fn test_unify_type_variable() {
        let mut infer = Infer::new();
        let var = infer.fresh();
        assert!(infer.unify(&var, &Type::Number, span()).is_ok());
        assert_eq!(infer.apply(&var), Type::Number);
    }

    #[test]
    fn test_unify_tuples() {
        let mut infer = Infer::new();
        let t1 = Type::Tuple(vec![Type::Number, Type::String]);
        let t2 = Type::Tuple(vec![Type::Number, Type::String]);
        assert!(infer.unify(&t1, &t2, span()).is_ok());
    }

    #[test]
    fn test_unify_tuples_mismatch() {
        let mut infer = Infer::new();
        let t1 = Type::Tuple(vec![Type::Number, Type::String]);
        let t2 = Type::Tuple(vec![Type::Number, Type::Bool]);
        assert!(infer.unify(&t1, &t2, span()).is_err());
    }

    #[test]
    fn test_unify_records() {
        let mut infer = Infer::new();
        let r1 = Type::record([("x", Type::Number), ("y", Type::String)]);
        let r2 = Type::record([("x", Type::Number), ("y", Type::String)]);
        assert!(infer.unify(&r1, &r2, span()).is_ok());
    }

    #[test]
    fn test_unify_functions() {
        let mut infer = Infer::new();
        let f1 = Type::function(vec![Type::Number], Type::String);
        let f2 = Type::function(vec![Type::Number], Type::String);
        assert!(infer.unify(&f1, &f2, span()).is_ok());
    }

    #[test]
    fn test_occurs_check() {
        let mut infer = Infer::new();
        let var = infer.fresh();
        // Try to unify 'a with ('a -> 'a), should fail
        let fn_ty = Type::function(vec![var.clone()], var.clone());
        assert!(infer.unify(&var, &fn_ty, span()).is_err());
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Ability unification tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_unify_empty_abilities() {
        let mut infer = Infer::new();
        let result = infer.unify_abilities(&AbilitySet::Empty, &AbilitySet::Empty, span());
        assert!(result.is_ok());
    }

    #[test]
    fn test_unify_same_abilities() {
        let mut infer = Infer::new();
        let a = AbilitySet::from_abilities([aid(1), aid(2)]);
        let b = AbilitySet::from_abilities([aid(1), aid(2)]);
        let result = infer.unify_abilities(&a, &b, span());
        assert!(result.is_ok());
    }

    #[test]
    fn test_unify_different_abilities_fails() {
        let mut infer = Infer::new();
        let a = AbilitySet::from_abilities([aid(1), aid(2)]);
        let b = AbilitySet::from_abilities([aid(1), aid(3)]);
        let result = infer.unify_abilities(&a, &b, span());
        assert!(result.is_err());
    }

    #[test]
    fn test_unify_ability_var_with_concrete() {
        let mut infer = Infer::new();
        let var = AbilitySet::var(0);
        let concrete = AbilitySet::from_abilities([aid(1), aid(2)]);
        let result = infer.unify_abilities(&var, &concrete, span());
        assert!(result.is_ok());

        // The variable should now be bound to the concrete set
        let applied = infer.apply_abilities(&var);
        assert_eq!(applied, concrete);
    }

    #[test]
    fn test_unify_ability_var_with_empty() {
        let mut infer = Infer::new();
        let var = AbilitySet::var(0);
        let result = infer.unify_abilities(&var, &AbilitySet::Empty, span());
        assert!(result.is_ok());

        let applied = infer.apply_abilities(&var);
        assert_eq!(applied, AbilitySet::Empty);
    }

    #[test]
    fn test_unify_same_ability_var() {
        let mut infer = Infer::new();
        let var = AbilitySet::var(0);
        let result = infer.unify_abilities(&var, &var, span());
        assert!(result.is_ok());
    }

    #[test]
    fn test_unify_rows_with_different_tails_is_most_general() {
        let mut infer = Infer::new();
        // {1 | t1} ~ {2 | t2}: both sides must resolve to {1, 2 | t3}, and
        // crucially t1 must absorb only {2 | t3} (not its own side's
        // abilities — the old unifier bound both tails to the full union,
        // over-widening every later use of the tails).
        let r1 = AbilitySet::row([aid(1)], 0);
        let r2 = AbilitySet::row([aid(2)], 1);
        infer.unify_abilities(&r1, &r2, span()).unwrap();

        let a1 = infer.apply_abilities(&r1);
        let a2 = infer.apply_abilities(&r2);
        assert_eq!(a1, a2, "unified rows must agree");
        assert_eq!(a1.concrete_abilities(), &[aid(1), aid(2)]);

        let t1 = infer.apply_abilities(&AbilitySet::var(0));
        assert!(
            !t1.contains(aid(1)),
            "t1 must not absorb its own side's abilities, got {t1}"
        );
        assert!(t1.contains(aid(2)));
    }

    #[test]
    fn test_apply_abilities() {
        let mut infer = Infer::new();
        let var = AbilitySet::var(0);
        let concrete = AbilitySet::from_abilities([aid(1), aid(2)]);

        // Unify the variable with concrete
        infer.unify_abilities(&var, &concrete, span()).unwrap();

        // Apply should resolve the variable
        let applied = infer.apply_abilities(&var);
        assert_eq!(applied, concrete);

        // Applying to an unbound variable returns the variable
        let unbound = AbilitySet::var(99);
        let applied_unbound = infer.apply_abilities(&unbound);
        assert_eq!(applied_unbound, unbound);
    }

    #[test]
    fn test_unify_functions_with_abilities() {
        let mut infer = Infer::new();

        let f1 = Type::function_with_abilities(
            vec![Type::Number],
            Type::String,
            AbilitySet::from_abilities([aid(1)]),
        );

        let f2 = Type::function_with_abilities(
            vec![Type::Number],
            Type::String,
            AbilitySet::from_abilities([aid(1)]),
        );

        let result = infer.unify(&f1, &f2, span());
        assert!(result.is_ok());
    }

    #[test]
    fn test_unify_functions_different_abilities_fails() {
        let mut infer = Infer::new();

        let f1 = Type::function_with_abilities(
            vec![Type::Number],
            Type::String,
            AbilitySet::from_abilities([aid(1)]),
        );

        let f2 = Type::function_with_abilities(
            vec![Type::Number],
            Type::String,
            AbilitySet::from_abilities([aid(2)]),
        );

        let result = infer.unify(&f1, &f2, span());
        assert!(result.is_err());
    }

    #[test]
    fn test_unify_ability_values() {
        let mut infer = Infer::new();

        let av1 = Type::ability_value(Type::String, AbilitySet::single(aid(1)));
        let av2 = Type::ability_value(Type::String, AbilitySet::single(aid(1)));

        let result = infer.unify(&av1, &av2, span());
        assert!(result.is_ok());
    }

    #[test]
    fn test_unify_ability_values_different_result_fails() {
        let mut infer = Infer::new();

        let av1 = Type::ability_value(Type::String, AbilitySet::single(aid(1)));
        let av2 = Type::ability_value(Type::Number, AbilitySet::single(aid(1)));

        let result = infer.unify(&av1, &av2, span());
        assert!(result.is_err());
    }

    #[test]
    fn test_generalize_with_ability_vars() {
        let infer = Infer::new();
        let env = TypeEnv::new();

        // A function type with an ability variable
        let ty =
            Type::function_with_abilities(vec![Type::var(0)], Type::var(0), AbilitySet::var(1));

        let scheme = infer.generalize(&env, &ty);

        // Both the type variable and ability variable should be quantified
        assert_eq!(scheme.vars, vec![0]);
        assert_eq!(scheme.ability_vars, vec![1]);
    }

    #[test]
    fn test_instantiate_with_ability_vars() {
        let mut infer = Infer::new();

        // Use higher IDs in the scheme so that fresh vars will be different
        let scheme = Scheme::poly_with_abilities(
            vec![100],
            vec![100],
            Type::function_with_abilities(
                vec![Type::var(100)],
                Type::var(100),
                AbilitySet::var(100),
            ),
        );

        let ty = infer.instantiate(&scheme);

        // Should get fresh type and ability variables (different from the scheme's 100s)
        if let Type::Function(f) = ty {
            assert!(matches!(f.params[0], Type::Var(id) if id != 100));
            assert!(matches!(f.abilities, AbilitySet::Var(id) if id != 100));
        } else {
            panic!("Expected function type");
        }
    }
}

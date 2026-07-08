//! Ability-set unification: finding a most general unifier for two effect
//! rows (concrete abilities, variables, and open row tails).

use crate::infer::{Infer, InferResult, TypeErrorKind, type_error};
use crate::types::{AbilitySet, AbilityVarId};

impl Infer {
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
}

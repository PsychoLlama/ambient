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
use crate::types::{NamedType, Type, TypeVarId};

use uuid::Uuid;

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
            // Reflexive atoms and error types. The primitives
            // (`Bool`/`Number`/`String`/`Binary`) are `extern` `Nominal` types
            // carrying a reserved uuid, so they unify through the
            // `(Nominal, Nominal)` arm below (uuid + structure), not here.
            (Type::Unit, Type::Unit)
            | (Type::Never, Type::Never)
            | (Type::Error, _)
            | (_, Type::Error) => Ok(()),

            // Rigid type parameters: an atom that unifies only with the
            // identically-named parameter. A `Param` meeting a concrete type,
            // an inference var (handled by the var arm below), or a *different*
            // parameter is a genuine mismatch, so everything else falls
            // through to the mismatch arm. `Param` never carries `args`, so it
            // never reaches the alias-expansion arms further down (which guard
            // on `Type::Named`).
            (Type::Param(a), Type::Param(b)) if a == b => Ok(()),

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

            // Handler types (Milestone 13): the ability must match, then the
            // answer types unify. Two handlers for different abilities are
            // distinct types; same ability with a free answer var absorbs the
            // other's answer.
            (Type::Handler(h1), Type::Handler(h2)) => {
                if h1.ability != h2.ability {
                    return Err(type_error(
                        TypeErrorKind::TypeMismatch {
                            expected: t1.clone(),
                            actual: t2.clone(),
                        },
                        span,
                    ));
                }
                self.unify(&h1.answer, &h2.answer, span)
            }

            // Named types. The head name and arity must match, and nominal
            // identities (every enum's uuid) must agree. A `None` uuid on a
            // *registered* enum name is resolved to that enum's canonical uuid
            // first, so an as-yet-unresolved annotation or a self-referential
            // payload (which arrive carrying `None`) unify strictly with the
            // resolved, uuid-carrying form — while two genuinely distinct
            // enums, even same-named ones from different packages, never unify.
            // A `None` that is *not* a registered enum (a type parameter, or a
            // container annotation that never passed through `resolve_holes`)
            // stays `None`; it still unifies with the uuid-carrying form,
            // because a conflict needs *both* sides to name a uuid.
            (Type::Named(n1), Type::Named(n2)) => {
                let id1 = self.resolve_named_identity(n1);
                let id2 = self.resolve_named_identity(n2);
                let identity_conflict = matches!(
                    (id1, id2),
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

            // A bare reference to a `type` alias can meet the alias's expanded
            // form: a `unique type` resolves to a `Type::Nominal`, a plain
            // alias to its target. `resolve_holes` expands aliases everywhere
            // it has the alias table, so the two spellings normally never meet
            // — but an ability method signature is resolved *before* the table
            // is populated (see `resolve_ability_def`), so its `Duration`
            // parameter stays an unexpanded `Named` and reaches here against a
            // caller's resolved `Nominal`. Expand and retry. The earlier
            // (Named, Named) arm already consumed name-vs-name, so the other
            // side here is never itself a `Named`; the guard skips enums (which
            // resolve to a uuid-carrying `Named`, not an alias) and generics.
            (Type::Named(n), _)
                if n.args.is_empty()
                    && n.uuid.is_none()
                    && self.expand_named_alias(&n.name).is_some() =>
            {
                match self.expand_named_alias(&n.name) {
                    Some(expanded) => self.unify(&expanded, &t2, span),
                    None => Ok(()), // unreachable: the guard proved it present
                }
            }
            (_, Type::Named(n))
                if n.args.is_empty()
                    && n.uuid.is_none()
                    && self.expand_named_alias(&n.name).is_some() =>
            {
                match self.expand_named_alias(&n.name) {
                    Some(expanded) => self.unify(&t1, &expanded, span),
                    None => Ok(()), // unreachable: the guard proved it present
                }
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

    /// Resolve a named type's nominal identity for unification.
    ///
    /// If the type already carries a uuid, use it. Otherwise, if its head name
    /// is a registered enum (including the reserved-name prelude enums
    /// `Option`/`Result`), fall back to that enum's canonical uuid — so an
    /// unresolved reference compares strictly against the resolved form. A name
    /// that is not a registered enum (a type parameter, or a structural
    /// container like `List`) has no identity and stays `None`.
    fn resolve_named_identity(&self, n: &NamedType) -> Option<Uuid> {
        n.uuid
            .or_else(|| self.enum_registry.get(&n.name).and_then(|info| info.uuid))
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
            Type::Handler(h) => self.occurs(var, &h.answer),
            _ => false,
        }
    }
}

mod abilities;
mod apply;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::infer::Scheme;
    use crate::infer::env::TypeEnv;
    use crate::types::{AbilityId, AbilitySet};

    /// A distinct, recognizable `AbilityId` for tests.
    fn aid(n: u8) -> AbilityId {
        AbilityId::from_bytes([n; 32])
    }

    fn span() -> (u32, u32) {
        (0, 0)
    }

    #[test]
    fn test_unify_primitives() {
        let mut infer = Infer::new();
        assert!(
            infer
                .unify(&Type::number(), &Type::number(), span())
                .is_ok()
        );
        assert!(
            infer
                .unify(&Type::string(), &Type::string(), span())
                .is_ok()
        );
        assert!(infer.unify(&Type::bool(), &Type::bool(), span()).is_ok());
        assert!(infer.unify(&Type::Unit, &Type::Unit, span()).is_ok());
    }

    #[test]
    fn test_unify_mismatch() {
        let mut infer = Infer::new();
        assert!(
            infer
                .unify(&Type::number(), &Type::string(), span())
                .is_err()
        );
        assert!(infer.unify(&Type::bool(), &Type::number(), span()).is_err());
    }

    #[test]
    fn test_unify_option_resolves_none_identity_to_canonical() {
        // A named reference to `Option` that arrives without a uuid (e.g. an
        // unresolved annotation) still unifies with the resolved, uuid-carrying
        // form — the registry fallback resolves the `None` to `OPTION_UUID`.
        let mut infer = Infer::new();
        let unresolved = Type::named("Option", vec![Type::number()]);
        assert!(unresolved.as_option().is_some());
        assert!(matches!(&unresolved, Type::Named(n) if n.uuid.is_none()));
        assert!(
            infer
                .unify(&unresolved, &Type::option(Type::number()), span())
                .is_ok()
        );
    }

    #[test]
    fn test_unify_option_rejects_foreign_identity() {
        // A look-alike enum that shares the name `Option` but carries a
        // *different* uuid must not unify with the real `Option` — this is the
        // "None hole" being closed.
        use crate::types::NamedType;
        let mut infer = Infer::new();
        let foreign = Type::Named(NamedType::with_identity(
            "Option",
            vec![Type::number()],
            Some(Uuid::from_u128(0x1234)),
        ));
        assert!(
            infer
                .unify(&foreign, &Type::option(Type::number()), span())
                .is_err()
        );
    }

    #[test]
    fn test_unify_named_alias_bridges_to_nominal() {
        // A `unique type` alias resolves to a `Type::Nominal`, but an ability
        // method signature's reference to it stays an unexpanded `Named` (it
        // is resolved before the alias table exists). The two must still
        // unify, in either order.
        let mut infer = Infer::new();
        let duration = Type::nominal(
            Uuid::from_u128(0x0003),
            Type::record([("secs", Type::number()), ("nanos", Type::number())]),
            Some("Duration"),
        );
        infer.register_type_alias(Arc::from("Duration"), duration.clone());

        let unexpanded = Type::named("Duration", vec![]);
        assert!(infer.unify(&unexpanded, &duration, span()).is_ok());
        assert!(infer.unify(&duration, &unexpanded, span()).is_ok());
    }

    #[test]
    fn test_unify_named_alias_still_rejects_wrong_type() {
        // The bridge only unifies the alias against its true expansion; a
        // mismatching argument (a bare `number` where a `Duration` is wanted)
        // is still an error.
        let mut infer = Infer::new();
        let duration = Type::nominal(
            Uuid::from_u128(0x0003),
            Type::record([("secs", Type::number()), ("nanos", Type::number())]),
            Some("Duration"),
        );
        infer.register_type_alias(Arc::from("Duration"), duration);

        let unexpanded = Type::named("Duration", vec![]);
        assert!(infer.unify(&unexpanded, &Type::number(), span()).is_err());
    }

    #[test]
    fn test_unify_type_variable() {
        let mut infer = Infer::new();
        let var = infer.fresh();
        assert!(infer.unify(&var, &Type::number(), span()).is_ok());
        assert_eq!(infer.apply(&var), Type::number());
    }

    #[test]
    fn test_unify_tuples() {
        let mut infer = Infer::new();
        let t1 = Type::Tuple(vec![Type::number(), Type::string()]);
        let t2 = Type::Tuple(vec![Type::number(), Type::string()]);
        assert!(infer.unify(&t1, &t2, span()).is_ok());
    }

    #[test]
    fn test_unify_tuples_mismatch() {
        let mut infer = Infer::new();
        let t1 = Type::Tuple(vec![Type::number(), Type::string()]);
        let t2 = Type::Tuple(vec![Type::number(), Type::bool()]);
        assert!(infer.unify(&t1, &t2, span()).is_err());
    }

    #[test]
    fn test_unify_records() {
        let mut infer = Infer::new();
        let r1 = Type::record([("x", Type::number()), ("y", Type::string())]);
        let r2 = Type::record([("x", Type::number()), ("y", Type::string())]);
        assert!(infer.unify(&r1, &r2, span()).is_ok());
    }

    #[test]
    fn test_unify_functions() {
        let mut infer = Infer::new();
        let f1 = Type::function(vec![Type::number()], Type::string());
        let f2 = Type::function(vec![Type::number()], Type::string());
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
            vec![Type::number()],
            Type::string(),
            AbilitySet::from_abilities([aid(1)]),
        );

        let f2 = Type::function_with_abilities(
            vec![Type::number()],
            Type::string(),
            AbilitySet::from_abilities([aid(1)]),
        );

        let result = infer.unify(&f1, &f2, span());
        assert!(result.is_ok());
    }

    #[test]
    fn test_unify_functions_different_abilities_fails() {
        let mut infer = Infer::new();

        let f1 = Type::function_with_abilities(
            vec![Type::number()],
            Type::string(),
            AbilitySet::from_abilities([aid(1)]),
        );

        let f2 = Type::function_with_abilities(
            vec![Type::number()],
            Type::string(),
            AbilitySet::from_abilities([aid(2)]),
        );

        let result = infer.unify(&f1, &f2, span());
        assert!(result.is_err());
    }

    #[test]
    fn test_unify_ability_values() {
        let mut infer = Infer::new();

        let av1 = Type::ability_value(Type::string(), AbilitySet::single(aid(1)));
        let av2 = Type::ability_value(Type::string(), AbilitySet::single(aid(1)));

        let result = infer.unify(&av1, &av2, span());
        assert!(result.is_ok());
    }

    #[test]
    fn test_unify_ability_values_different_result_fails() {
        let mut infer = Infer::new();

        let av1 = Type::ability_value(Type::string(), AbilitySet::single(aid(1)));
        let av2 = Type::ability_value(Type::number(), AbilitySet::single(aid(1)));

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

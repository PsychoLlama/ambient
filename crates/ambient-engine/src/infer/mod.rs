//! Type inference for the Ambient language.
//!
//! This module implements Hindley-Milner type inference with:
//! - Algorithm W for principal type inference
//! - Unification with occurs check
//! - Let-polymorphism (generalization at let bindings)
//! - Type environment with lexical scoping
//!
//! # Algorithm W Overview
//!
//! The inference algorithm works in two phases:
//!
//! 1. **Constraint Generation**: Traverse the AST, assigning fresh type variables
//!    to expressions and collecting equality constraints between types.
//!
//! 2. **Unification**: Solve constraints by finding a most general unifier (MGU).
//!    Uses substitution to replace type variables with concrete types.
//!
//! ## Key Operations
//!
//! - **`infer_expr`**: Infers the type of an expression, returning constraints
//! - **`unify`**: Unifies two types, updating the substitution map
//! - **`generalize`**: Converts a type to a polymorphic scheme (∀-quantified)
//! - **`instantiate`**: Creates fresh type variables for a polymorphic scheme
//!
//! ## Example: Let Polymorphism
//!
//! ```text
//! let id = |x| x;    // id : ∀a. a -> a
//! let a = id(42);    // instantiate: number -> number
//! let b = id(true);  // instantiate: bool -> bool
//! ```
//!
//! The identity function `id` is generalized at its binding site, allowing
//! it to be used at different types.
//!
//! # Ability Tracking
//!
//! The type system tracks algebraic effects through ability sets on function
//! types. A function `fn(): number / Console` can perform Console operations.
//! The inference engine propagates these ability requirements and checks that
//! all abilities are handled.
//!
//! # Module Organization
//!
//! - [`error`] - Type error types and display implementations
//! - [`env`] - Type environment (`TypeEnv`) and type schemes (`Scheme`)
//! - [`check`] - Module-level type checking (`check_module`)
//! - [`Infer`] - The main type inference engine

mod check;
mod env;
mod error;

pub use check::{check_module, check_module_with_registry, CheckResult};
pub use env::{Scheme, TypeEnv};
pub use error::{BoxedTypeError, BoxedTypeErrorExt, InferResult, TypeError, TypeErrorKind};

use error::type_error;

use std::collections::HashMap;
use std::sync::Arc;

use crate::ability_resolver::{AbilityResolver, EngineTypeFactory};
use crate::ast::{Expr, ExprKind, Pattern, PatternKind, StmtKind, UnaryOp};
use crate::types::{
    AbilityId, AbilityRegistry, AbilitySet, AbilityValueType, AbilityVarId, ForallType,
    FunctionType, NamedType, NominalType, RecordType, Type, TypeVar, TypeVarGen, TypeVarId,
};

// ─────────────────────────────────────────────────────────────────────────────
// Type Inference
// ─────────────────────────────────────────────────────────────────────────────

/// Type inference context.
pub struct Infer {
    /// Type variable generator.
    gen: TypeVarGen,
    /// Substitution mapping type variables to their bindings.
    subst: HashMap<TypeVarId, Type>,
    /// Substitution mapping ability variables to their bindings (Milestone 8).
    ability_subst: HashMap<AbilityVarId, AbilitySet>,
    /// Current ability requirements being accumulated (Milestone 8).
    /// This tracks abilities used in the current function being inferred.
    current_abilities: AbilitySet,
    /// Optional ability registry for dependency tracking.
    ability_registry: Option<AbilityRegistry>,
    /// Ability resolver for looking up ability and method information.
    ability_resolver: AbilityResolver,
}

impl Default for Infer {
    fn default() -> Self {
        Self::new()
    }
}

impl Infer {
    /// Create a new inference context with standard abilities.
    #[must_use]
    pub fn new() -> Self {
        Self {
            gen: TypeVarGen::new(),
            subst: HashMap::new(),
            ability_subst: HashMap::new(),
            current_abilities: AbilitySet::Empty,
            ability_registry: None,
            ability_resolver: crate::ability_resolver::standard_abilities(),
        }
    }

    /// Create a new inference context with an ability registry.
    #[must_use]
    pub fn with_registry(registry: AbilityRegistry) -> Self {
        Self {
            gen: TypeVarGen::new(),
            subst: HashMap::new(),
            ability_subst: HashMap::new(),
            current_abilities: AbilitySet::Empty,
            ability_registry: Some(registry),
            ability_resolver: crate::ability_resolver::standard_abilities(),
        }
    }

    /// Create a new inference context with a custom ability resolver.
    #[must_use]
    pub fn with_resolver(resolver: AbilityResolver) -> Self {
        Self {
            gen: TypeVarGen::new(),
            subst: HashMap::new(),
            ability_subst: HashMap::new(),
            current_abilities: AbilitySet::Empty,
            ability_registry: None,
            ability_resolver: resolver,
        }
    }

    /// Generate a fresh type variable.
    pub fn fresh(&mut self) -> Type {
        self.gen.fresh()
    }

    /// Generate a fresh ability variable.
    pub fn fresh_ability_var(&mut self) -> AbilitySet {
        self.gen.fresh_ability_var()
    }

    /// Add an ability to the current requirements, including its dependencies.
    pub fn require_ability(&mut self, ability: AbilityId) {
        // Add the ability and all its dependencies
        let abilities = if let Some(registry) = &self.ability_registry {
            registry.ability_with_dependencies(ability)
        } else {
            AbilitySet::single(ability)
        };
        self.current_abilities = self.current_abilities.union(&abilities);
    }

    /// Add an ability set to the current requirements.
    pub fn require_abilities(&mut self, abilities: &AbilitySet) {
        self.current_abilities = self.current_abilities.union(abilities);
    }

    /// Get the current ability requirements.
    #[must_use]
    pub fn current_abilities(&self) -> &AbilitySet {
        &self.current_abilities
    }

    /// Reset ability tracking (e.g., when entering a new function body).
    pub fn reset_abilities(&mut self) {
        self.current_abilities = AbilitySet::Empty;
    }

    /// Resolve type holes (`_`) in a type annotation by replacing them with fresh
    /// type variables. This enables partial annotation where users can specify
    /// some parts of a type and let inference determine the rest.
    pub fn resolve_holes(&mut self, ty: &Type) -> Type {
        match ty {
            Type::Hole => self.fresh(),
            Type::Tuple(elems) => {
                Type::Tuple(elems.iter().map(|e| self.resolve_holes(e)).collect())
            }
            Type::Record(rec) => Type::Record(RecordType::new(
                rec.fields
                    .iter()
                    .map(|(n, t)| (n.clone(), self.resolve_holes(t)))
                    .collect(),
            )),
            Type::Function(f) => {
                let params = f.params.iter().map(|p| self.resolve_holes(p)).collect();
                let ret = self.resolve_holes(&f.ret);
                Type::function_with_abilities(params, ret, f.abilities.clone())
            }
            Type::Named(n) => Type::Named(NamedType::new(
                n.name.clone(),
                n.args.iter().map(|a| self.resolve_holes(a)).collect(),
            )),
            Type::Nominal(n) => Type::Nominal(NominalType::new(
                n.uuid,
                self.resolve_holes(&n.inner),
                n.name.clone(),
            )),
            Type::AbilityValue(av) => Type::AbilityValue(AbilityValueType::new(
                self.resolve_holes(&av.result),
                av.ability.clone(),
            )),
            Type::Forall(f) => Type::Forall(ForallType::with_abilities(
                f.vars.clone(),
                f.ability_vars.clone(),
                self.resolve_holes(&f.body),
            )),
            // Other types remain unchanged
            _ => ty.clone(),
        }
    }

    /// Apply substitution to a type.
    #[must_use]
    pub fn apply(&self, ty: &Type) -> Type {
        self.apply_impl(ty, &mut Vec::new())
    }

    /// Apply ability substitution to an ability set.
    #[must_use]
    pub fn apply_abilities(&self, abilities: &AbilitySet) -> AbilitySet {
        match abilities {
            AbilitySet::Empty | AbilitySet::Concrete(_) => abilities.clone(),
            AbilitySet::Var(id) => self
                .ability_subst
                .get(id)
                .cloned()
                .unwrap_or_else(|| abilities.clone()),
            AbilitySet::Row { concrete, tail } => {
                if let Some(tail_set) = self.ability_subst.get(tail) {
                    AbilitySet::from_abilities(concrete.iter().copied()).union(tail_set)
                } else {
                    abilities.clone()
                }
            }
        }
    }

    fn apply_impl(&self, ty: &Type, seen: &mut Vec<TypeVarId>) -> Type {
        match ty {
            Type::Var(TypeVar::Unbound(id)) => {
                if seen.contains(id) {
                    return ty.clone(); // Cycle, stop
                }
                if let Some(bound) = self.subst.get(id) {
                    seen.push(*id);
                    let result = self.apply_impl(bound, seen);
                    seen.pop();
                    result
                } else {
                    ty.clone()
                }
            }
            Type::Var(TypeVar::Link(link)) => self.apply_impl(&link.borrow(), seen),
            Type::Tuple(elems) => {
                Type::Tuple(elems.iter().map(|e| self.apply_impl(e, seen)).collect())
            }
            Type::Record(r) => Type::Record(RecordType::new(
                r.fields
                    .iter()
                    .map(|(n, t)| (n.clone(), self.apply_impl(t, seen)))
                    .collect(),
            )),
            Type::Function(f) => {
                let applied_abilities = self.apply_abilities(&f.abilities);
                Type::Function(FunctionType::with_abilities(
                    f.params.iter().map(|p| self.apply_impl(p, seen)).collect(),
                    self.apply_impl(&f.ret, seen),
                    applied_abilities,
                ))
            }
            Type::Named(n) => Type::Named(crate::types::NamedType::new(
                n.name.clone(),
                n.args.iter().map(|a| self.apply_impl(a, seen)).collect(),
            )),
            Type::Nominal(n) => Type::Nominal(crate::types::NominalType::new(
                n.uuid,
                self.apply_impl(&n.inner, seen),
                n.name.clone(),
            )),
            Type::AbilityValue(av) => {
                let applied_ability = self.apply_abilities(&av.ability);
                Type::AbilityValue(AbilityValueType::new(
                    self.apply_impl(&av.result, seen),
                    applied_ability,
                ))
            }
            Type::Forall(f) => {
                // Don't apply subst to bound variables
                let mut new_subst = self.subst.clone();
                for var in &f.vars {
                    new_subst.remove(var);
                }
                let mut new_ability_subst = self.ability_subst.clone();
                for var in &f.ability_vars {
                    new_ability_subst.remove(var);
                }
                let inner_infer = Infer {
                    gen: TypeVarGen::new(),
                    subst: new_subst,
                    ability_subst: new_ability_subst,
                    current_abilities: AbilitySet::Empty,
                    ability_registry: self.ability_registry.clone(),
                    ability_resolver: crate::ability_resolver::standard_abilities(),
                };
                Type::Forall(crate::types::ForallType::with_abilities(
                    f.vars.clone(),
                    f.ability_vars.clone(),
                    inner_infer.apply(&f.body),
                ))
            }
            _ => ty.clone(),
        }
    }

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
            | (Type::Never, Type::Never)
            | (Type::Error, _)
            | (_, Type::Error) => Ok(()),

            // Type variables
            (Type::Var(TypeVar::Unbound(id1)), Type::Var(TypeVar::Unbound(id2))) if id1 == id2 => {
                Ok(())
            }
            (Type::Var(TypeVar::Unbound(id)), ty) | (ty, Type::Var(TypeVar::Unbound(id))) => {
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

            // Named types
            (Type::Named(n1), Type::Named(n2)) => {
                if n1.name != n2.name || n1.args.len() != n2.args.len() {
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
    fn occurs(&self, var: TypeVarId, ty: &Type) -> bool {
        let ty = self.apply(ty);
        match ty {
            Type::Var(TypeVar::Unbound(id)) => id == var,
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
                    // Different tails - need to create a fresh tail for the common part
                    // For now, we handle the simple case where one contains the other
                    let fresh_tail = self.gen.fresh_ability_id();

                    // The common abilities plus the fresh tail
                    let mut all_abilities: Vec<_> = c1.iter().chain(c2.iter()).copied().collect();
                    all_abilities.sort_unstable();
                    all_abilities.dedup();

                    let new_row = AbilitySet::Row {
                        concrete: all_abilities,
                        tail: fresh_tail,
                    };

                    self.ability_subst.insert(*t1, new_row.clone());
                    self.ability_subst.insert(*t2, new_row);
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
    fn ability_occurs(&self, var: AbilityVarId, abilities: &AbilitySet) -> bool {
        let abilities = self.apply_abilities(abilities);
        match &abilities {
            AbilitySet::Empty | AbilitySet::Concrete(_) => false,
            AbilitySet::Var(id) => *id == var,
            AbilitySet::Row { tail, .. } => *tail == var,
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Ability lookup helpers (Milestone 8)
    // ─────────────────────────────────────────────────────────────────────────

    /// Well-known ability ID for Async (needed for special polymorphic handling).
    const ABILITY_ASYNC: AbilityId = 0x0005;

    /// Convert an ability name to its ID using the resolver.
    fn ability_name_to_id(&self, name: &str) -> Option<AbilityId> {
        self.ability_resolver.name_to_id(name)
    }

    /// Convert an ability ID to its name using the resolver.
    fn ability_id_to_name(&self, id: AbilityId) -> Option<&str> {
        self.ability_resolver.id_to_name(id)
    }

    /// Get the method signatures for an ability using the resolver.
    ///
    /// Returns a list of (`method_name`, `param_count`, `return_type`) tuples.
    fn get_ability_method_signatures(&self, ability_id: AbilityId) -> Vec<(String, usize, Type)> {
        let factory = EngineTypeFactory;
        self.ability_resolver
            .get_method_signatures(ability_id, &factory)
    }

    /// Try to infer which ability a handler literal is for based on method names.
    ///
    /// Returns the ability ID if all methods belong to exactly one ability.
    fn infer_ability_from_methods(&self, method_names: &[Arc<str>]) -> Option<AbilityId> {
        self.ability_resolver
            .infer_ability_from_methods(method_names)
    }

    /// Try to infer the type of an intrinsic function call.
    ///
    /// Returns `Some(return_type)` if the name is a known intrinsic,
    /// `None` if it should be handled as a regular function call.
    ///
    /// Intrinsics must be called with their full qualified path:
    /// - `core.math.sqrt`, `core.math.abs`, etc.
    /// - `core.list.length`, `core.list.head`, etc.
    /// - `core.string.length`, `core.string.split`, etc.
    /// - `core.map.empty`, `core.map.get`, etc.
    /// - `core.set.empty`, `core.set.insert`, etc.
    /// - `core.option.unwrap_or`, `core.option.is_some`, etc.
    /// - `core.result.is_ok`, `core.result.is_err`, etc.
    /// - `core.convert.to_string`, `core.convert.parse_number`, etc.
    #[allow(clippy::too_many_lines)]
    fn try_infer_intrinsic(
        &mut self,
        env: &TypeEnv,
        qualified_name: &crate::ast::QualifiedName,
        args: &mut [Expr],
        span: (u32, u32),
    ) -> InferResult<Option<Type>> {
        // Helper to create List type
        let list_of = |elem: Type| Type::named("List", vec![elem]);

        // Convert path to slice for matching
        let path: Vec<&str> = qualified_name.path.iter().map(AsRef::as_ref).collect();
        let name = qualified_name.name.as_ref();

        match (path.as_slice(), name) {
            // ─────────────────────────────────────────────────────────────────
            // core.math - Math intrinsics
            // ─────────────────────────────────────────────────────────────────
            (
                ["core", "math"],
                "sqrt" | "abs" | "floor" | "ceil" | "round" | "trunc" | "sin" | "cos" | "tan"
                | "ln" | "exp" | "asin" | "acos" | "atan" | "log10" | "log2",
            ) if args.len() == 1 => {
                let arg_ty = self.infer_expr(env, &mut args[0])?;
                self.unify(&arg_ty, &Type::Number, span)?;
                Ok(Some(Type::Number))
            }
            (["core", "math"], "pow" | "min" | "max" | "atan2") if args.len() == 2 => {
                let a_ty = self.infer_expr(env, &mut args[0])?;
                let b_ty = self.infer_expr(env, &mut args[1])?;
                self.unify(&a_ty, &Type::Number, span)?;
                self.unify(&b_ty, &Type::Number, span)?;
                Ok(Some(Type::Number))
            }

            // ─────────────────────────────────────────────────────────────────
            // core.list - List operations
            // ─────────────────────────────────────────────────────────────────
            (["core", "list"], "length") if args.len() == 1 => {
                let list_ty = self.infer_expr(env, &mut args[0])?;
                let elem_ty = self.fresh();
                self.unify(&list_ty, &list_of(elem_ty), span)?;
                Ok(Some(Type::Number))
            }
            (["core", "list"], "get") if args.len() == 2 => {
                let list_ty = self.infer_expr(env, &mut args[0])?;
                let index_ty = self.infer_expr(env, &mut args[1])?;
                let elem_ty = self.fresh();
                self.unify(&list_ty, &list_of(elem_ty.clone()), span)?;
                self.unify(&index_ty, &Type::Number, span)?;
                // Returns the element or Unit for out of bounds
                // For now, return the element type (assuming in bounds)
                Ok(Some(elem_ty))
            }
            (["core", "list"], "head") if args.len() == 1 => {
                let list_ty = self.infer_expr(env, &mut args[0])?;
                let elem_ty = self.fresh();
                self.unify(&list_ty, &list_of(elem_ty.clone()), span)?;
                // Returns the element or Unit for empty list
                Ok(Some(elem_ty))
            }
            (["core", "list"], "tail" | "reverse" | "sort") if args.len() == 1 => {
                let list_ty = self.infer_expr(env, &mut args[0])?;
                let elem_ty = self.fresh();
                self.unify(&list_ty, &list_of(elem_ty.clone()), span)?;
                Ok(Some(list_of(elem_ty)))
            }
            (["core", "list"], "concat") if args.len() == 2 => {
                let list1_ty = self.infer_expr(env, &mut args[0])?;
                let list2_ty = self.infer_expr(env, &mut args[1])?;
                let elem_ty = self.fresh();
                self.unify(&list1_ty, &list_of(elem_ty.clone()), span)?;
                self.unify(&list2_ty, &list_of(elem_ty.clone()), span)?;
                Ok(Some(list_of(elem_ty)))
            }
            (["core", "list"], "append") if args.len() == 2 => {
                let list_ty = self.infer_expr(env, &mut args[0])?;
                let elem_arg_ty = self.infer_expr(env, &mut args[1])?;
                let elem_ty = self.fresh();
                self.unify(&list_ty, &list_of(elem_ty.clone()), span)?;
                self.unify(&elem_arg_ty, &elem_ty, span)?;
                Ok(Some(list_of(elem_ty)))
            }
            (["core", "list"], "is_empty") if args.len() == 1 => {
                let list_ty = self.infer_expr(env, &mut args[0])?;
                let elem_ty = self.fresh();
                self.unify(&list_ty, &list_of(elem_ty), span)?;
                Ok(Some(Type::Bool))
            }
            (["core", "list"], "slice") if args.len() == 3 => {
                let list_ty = self.infer_expr(env, &mut args[0])?;
                let start_ty = self.infer_expr(env, &mut args[1])?;
                let end_ty = self.infer_expr(env, &mut args[2])?;
                let elem_ty = self.fresh();
                self.unify(&list_ty, &list_of(elem_ty.clone()), span)?;
                self.unify(&start_ty, &Type::Number, span)?;
                self.unify(&end_ty, &Type::Number, span)?;
                Ok(Some(list_of(elem_ty)))
            }

            // ─────────────────────────────────────────────────────────────────
            // core.string - String operations
            // ─────────────────────────────────────────────────────────────────
            (["core", "string"], "length") if args.len() == 1 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                self.unify(&str_ty, &Type::String, span)?;
                Ok(Some(Type::Number))
            }
            (["core", "string"], "concat") if args.len() == 2 => {
                let str1_ty = self.infer_expr(env, &mut args[0])?;
                let str2_ty = self.infer_expr(env, &mut args[1])?;
                self.unify(&str1_ty, &Type::String, span)?;
                self.unify(&str2_ty, &Type::String, span)?;
                Ok(Some(Type::String))
            }
            (["core", "string"], "contains" | "starts_with" | "ends_with") if args.len() == 2 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                let substr_ty = self.infer_expr(env, &mut args[1])?;
                self.unify(&str_ty, &Type::String, span)?;
                self.unify(&substr_ty, &Type::String, span)?;
                Ok(Some(Type::Bool))
            }
            (["core", "string"], "split") if args.len() == 2 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                let delim_ty = self.infer_expr(env, &mut args[1])?;
                self.unify(&str_ty, &Type::String, span)?;
                self.unify(&delim_ty, &Type::String, span)?;
                Ok(Some(list_of(Type::String)))
            }
            (["core", "string"], "join") if args.len() == 2 => {
                let list_ty = self.infer_expr(env, &mut args[0])?;
                let delim_ty = self.infer_expr(env, &mut args[1])?;
                self.unify(&list_ty, &list_of(Type::String), span)?;
                self.unify(&delim_ty, &Type::String, span)?;
                Ok(Some(Type::String))
            }
            (["core", "string"], "trim" | "to_upper" | "to_lower" | "reverse")
                if args.len() == 1 =>
            {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                self.unify(&str_ty, &Type::String, span)?;
                Ok(Some(Type::String))
            }
            (["core", "string"], "slice") if args.len() == 3 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                let start_ty = self.infer_expr(env, &mut args[1])?;
                let end_ty = self.infer_expr(env, &mut args[2])?;
                self.unify(&str_ty, &Type::String, span)?;
                self.unify(&start_ty, &Type::Number, span)?;
                self.unify(&end_ty, &Type::Number, span)?;
                Ok(Some(Type::String))
            }
            (["core", "string"], "chars") if args.len() == 1 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                self.unify(&str_ty, &Type::String, span)?;
                Ok(Some(list_of(Type::String)))
            }
            (["core", "string"], "replace") if args.len() == 3 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                let pattern_ty = self.infer_expr(env, &mut args[1])?;
                let replacement_ty = self.infer_expr(env, &mut args[2])?;
                self.unify(&str_ty, &Type::String, span)?;
                self.unify(&pattern_ty, &Type::String, span)?;
                self.unify(&replacement_ty, &Type::String, span)?;
                Ok(Some(Type::String))
            }
            (["core", "string"], "index_of") if args.len() == 2 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                let substr_ty = self.infer_expr(env, &mut args[1])?;
                self.unify(&str_ty, &Type::String, span)?;
                self.unify(&substr_ty, &Type::String, span)?;
                Ok(Some(Type::Number))
            }
            (["core", "string"], "repeat") if args.len() == 2 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                let count_ty = self.infer_expr(env, &mut args[1])?;
                self.unify(&str_ty, &Type::String, span)?;
                self.unify(&count_ty, &Type::Number, span)?;
                Ok(Some(Type::String))
            }

            // ─────────────────────────────────────────────────────────────────
            // core.convert - Type conversion
            // ─────────────────────────────────────────────────────────────────
            (["core", "convert"], "to_string") if args.len() == 1 => {
                // Accept any type
                let _arg_ty = self.infer_expr(env, &mut args[0])?;
                Ok(Some(Type::String))
            }
            (["core", "convert"], "parse_number") if args.len() == 1 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                self.unify(&str_ty, &Type::String, span)?;
                // Returns (bool, number) tuple
                Ok(Some(Type::tuple(vec![Type::Bool, Type::Number])))
            }
            (["core", "convert"], "parse_bool") if args.len() == 1 => {
                let str_ty = self.infer_expr(env, &mut args[0])?;
                self.unify(&str_ty, &Type::String, span)?;
                // Returns (bool, bool) tuple
                Ok(Some(Type::tuple(vec![Type::Bool, Type::Bool])))
            }

            // ─────────────────────────────────────────────────────────────────
            // core.map - Map operations
            // ─────────────────────────────────────────────────────────────────
            (["core", "map"], "empty") if args.is_empty() => {
                let key_ty = self.fresh();
                let val_ty = self.fresh();
                Ok(Some(Type::named("Map", vec![key_ty, val_ty])))
            }
            (["core", "map"], "get") if args.len() == 2 => {
                let map_ty = self.infer_expr(env, &mut args[0])?;
                let key_arg_ty = self.infer_expr(env, &mut args[1])?;
                let key_ty = self.fresh();
                let val_ty = self.fresh();
                self.unify(
                    &map_ty,
                    &Type::named("Map", vec![key_ty.clone(), val_ty.clone()]),
                    span,
                )?;
                self.unify(&key_arg_ty, &key_ty, span)?;
                // Returns Unit if key not found (should return Option)
                Ok(Some(val_ty))
            }
            (["core", "map"], "insert") if args.len() == 3 => {
                let map_ty = self.infer_expr(env, &mut args[0])?;
                let key_arg_ty = self.infer_expr(env, &mut args[1])?;
                let val_arg_ty = self.infer_expr(env, &mut args[2])?;
                let key_ty = self.fresh();
                let val_ty = self.fresh();
                self.unify(
                    &map_ty,
                    &Type::named("Map", vec![key_ty.clone(), val_ty.clone()]),
                    span,
                )?;
                self.unify(&key_arg_ty, &key_ty, span)?;
                self.unify(&val_arg_ty, &val_ty.clone(), span)?;
                Ok(Some(Type::named("Map", vec![key_ty, val_ty])))
            }
            (["core", "map"], "remove") if args.len() == 2 => {
                let map_ty = self.infer_expr(env, &mut args[0])?;
                let key_arg_ty = self.infer_expr(env, &mut args[1])?;
                let key_ty = self.fresh();
                let val_ty = self.fresh();
                self.unify(
                    &map_ty,
                    &Type::named("Map", vec![key_ty.clone(), val_ty.clone()]),
                    span,
                )?;
                self.unify(&key_arg_ty, &key_ty, span)?;
                Ok(Some(Type::named("Map", vec![key_ty, val_ty])))
            }
            (["core", "map"], "contains") if args.len() == 2 => {
                let map_ty = self.infer_expr(env, &mut args[0])?;
                let key_arg_ty = self.infer_expr(env, &mut args[1])?;
                let key_ty = self.fresh();
                let val_ty = self.fresh();
                self.unify(
                    &map_ty,
                    &Type::named("Map", vec![key_ty.clone(), val_ty]),
                    span,
                )?;
                self.unify(&key_arg_ty, &key_ty, span)?;
                Ok(Some(Type::Bool))
            }
            (["core", "map"], "length") if args.len() == 1 => {
                let map_ty = self.infer_expr(env, &mut args[0])?;
                let key_ty = self.fresh();
                let val_ty = self.fresh();
                self.unify(&map_ty, &Type::named("Map", vec![key_ty, val_ty]), span)?;
                Ok(Some(Type::Number))
            }
            (["core", "map"], "keys") if args.len() == 1 => {
                let map_ty = self.infer_expr(env, &mut args[0])?;
                let key_ty = self.fresh();
                let val_ty = self.fresh();
                self.unify(
                    &map_ty,
                    &Type::named("Map", vec![key_ty.clone(), val_ty]),
                    span,
                )?;
                Ok(Some(list_of(key_ty)))
            }
            (["core", "map"], "values") if args.len() == 1 => {
                let map_ty = self.infer_expr(env, &mut args[0])?;
                let key_ty = self.fresh();
                let val_ty = self.fresh();
                self.unify(
                    &map_ty,
                    &Type::named("Map", vec![key_ty, val_ty.clone()]),
                    span,
                )?;
                Ok(Some(list_of(val_ty)))
            }

            // ─────────────────────────────────────────────────────────────────
            // core.set - Set operations
            // ─────────────────────────────────────────────────────────────────
            (["core", "set"], "empty") if args.is_empty() => {
                let elem_ty = self.fresh();
                Ok(Some(Type::named("Set", vec![elem_ty])))
            }
            (["core", "set"], "insert" | "remove") if args.len() == 2 => {
                let set_ty = self.infer_expr(env, &mut args[0])?;
                let elem_arg_ty = self.infer_expr(env, &mut args[1])?;
                let elem_ty = self.fresh();
                self.unify(&set_ty, &Type::named("Set", vec![elem_ty.clone()]), span)?;
                self.unify(&elem_arg_ty, &elem_ty.clone(), span)?;
                Ok(Some(Type::named("Set", vec![elem_ty])))
            }
            (["core", "set"], "contains") if args.len() == 2 => {
                let set_ty = self.infer_expr(env, &mut args[0])?;
                let elem_arg_ty = self.infer_expr(env, &mut args[1])?;
                let elem_ty = self.fresh();
                self.unify(&set_ty, &Type::named("Set", vec![elem_ty.clone()]), span)?;
                self.unify(&elem_arg_ty, &elem_ty, span)?;
                Ok(Some(Type::Bool))
            }
            (["core", "set"], "length") if args.len() == 1 => {
                let set_ty = self.infer_expr(env, &mut args[0])?;
                let elem_ty = self.fresh();
                self.unify(&set_ty, &Type::named("Set", vec![elem_ty]), span)?;
                Ok(Some(Type::Number))
            }
            (["core", "set"], "union" | "intersection" | "difference") if args.len() == 2 => {
                let set1_ty = self.infer_expr(env, &mut args[0])?;
                let set2_ty = self.infer_expr(env, &mut args[1])?;
                let elem_ty = self.fresh();
                self.unify(&set1_ty, &Type::named("Set", vec![elem_ty.clone()]), span)?;
                self.unify(&set2_ty, &Type::named("Set", vec![elem_ty.clone()]), span)?;
                Ok(Some(Type::named("Set", vec![elem_ty])))
            }
            (["core", "set"], "to_list") if args.len() == 1 => {
                let set_ty = self.infer_expr(env, &mut args[0])?;
                let elem_ty = self.fresh();
                self.unify(&set_ty, &Type::named("Set", vec![elem_ty.clone()]), span)?;
                Ok(Some(list_of(elem_ty)))
            }

            // ─────────────────────────────────────────────────────────────────
            // core.option - Option operations
            // ─────────────────────────────────────────────────────────────────
            (["core", "option"], "unwrap_or") if args.len() == 2 => {
                let opt_ty = self.infer_expr(env, &mut args[0])?;
                let default_ty = self.infer_expr(env, &mut args[1])?;
                let inner_ty = self.fresh();
                self.unify(&opt_ty, &Type::option(inner_ty.clone()), span)?;
                self.unify(&default_ty, &inner_ty.clone(), span)?;
                Ok(Some(inner_ty))
            }
            (["core", "option"], "is_some" | "is_none") if args.len() == 1 => {
                let opt_ty = self.infer_expr(env, &mut args[0])?;
                let inner_ty = self.fresh();
                self.unify(&opt_ty, &Type::option(inner_ty), span)?;
                Ok(Some(Type::Bool))
            }

            // ─────────────────────────────────────────────────────────────────
            // core.result - Result operations
            // ─────────────────────────────────────────────────────────────────
            (["core", "result"], "is_ok" | "is_err") if args.len() == 1 => {
                let res_ty = self.infer_expr(env, &mut args[0])?;
                let ok_ty = self.fresh();
                let err_ty = self.fresh();
                self.unify(&res_ty, &Type::result(ok_ty, err_ty), span)?;
                Ok(Some(Type::Bool))
            }

            // ─────────────────────────────────────────────────────────────────
            // core.enum - Enum operations
            // ─────────────────────────────────────────────────────────────────
            (["core", "enum"], "tag") if args.len() == 1 => {
                // Accept any enum type
                let _enum_ty = self.infer_expr(env, &mut args[0])?;
                Ok(Some(Type::Number))
            }
            (["core", "enum"], "payload") if args.len() == 1 => {
                // Accept any enum type, return the payload type
                // This is tricky - we'd need to know which variant
                // For now, return a fresh type variable
                let _enum_ty = self.infer_expr(env, &mut args[0])?;
                let payload_ty = self.fresh();
                Ok(Some(payload_ty))
            }

            _ => Ok(None),
        }
    }

    /// Look up an ability method and return its ID, result type, and additional abilities to require.
    ///
    /// For most abilities, the additional abilities set is empty. For `Async.all` and `Async.race`,
    /// it includes the underlying ability from the suspended ability values being performed.
    fn lookup_ability_method(
        &mut self,
        ability_name: &str,
        method_name: &str,
        arg_tys: &[Type],
        span: (u32, u32),
    ) -> InferResult<(AbilityId, Type, AbilitySet)> {
        let ability_id = self.ability_name_to_id(ability_name).ok_or_else(|| {
            type_error(
                TypeErrorKind::UnknownAbility {
                    name: ability_name.into(),
                },
                span,
            )
        })?;

        // Special handling for Async methods which are polymorphic
        if ability_id == Self::ABILITY_ASYNC {
            let (result_ty, additional_abilities) = match method_name {
                "all" => {
                    // Async.all: List<Ability<T, A!>> -> List<T> with Async, A
                    self.infer_async_all_type(arg_tys, span)?
                }
                "race" => {
                    // Async.race: List<Ability<T, A!>> -> T with Async, A
                    self.infer_async_race_type(arg_tys, span)?
                }
                _ => {
                    return Err(type_error(
                        TypeErrorKind::UnknownAbilityMethod {
                            ability: ability_name.into(),
                            method: method_name.into(),
                        },
                        span,
                    ))
                }
            };
            return Ok((ability_id, result_ty, additional_abilities));
        }

        // For other abilities, look up the return type from the resolver
        let factory = EngineTypeFactory;
        let result_ty = self
            .ability_resolver
            .get_method_return_type(ability_name, method_name, &factory)
            .ok_or_else(|| {
                type_error(
                    TypeErrorKind::UnknownAbilityMethod {
                        ability: ability_name.into(),
                        method: method_name.into(),
                    },
                    span,
                )
            })?;

        Ok((ability_id, result_ty, AbilitySet::Empty))
    }

    /// Infer the result type for `Async.all(ops)` where `ops: List<Ability<T, A!>>`.
    /// Returns `(List<T>, A)` - the result type and the underlying ability to require.
    fn infer_async_all_type(
        &mut self,
        arg_tys: &[Type],
        span: (u32, u32),
    ) -> InferResult<(Type, AbilitySet)> {
        // Async.all takes exactly one argument
        if arg_tys.len() != 1 {
            return Err(type_error(
                TypeErrorKind::ArityMismatch {
                    expected: 1,
                    actual: arg_tys.len(),
                },
                span,
            ));
        }

        // Extract T and A from List<Ability<T, A!>>
        let (element_result_ty, underlying_ability) =
            self.extract_list_ability_types(&arg_tys[0], span)?;

        // Return List<T>
        let result_ty = Type::named("List", vec![element_result_ty]);
        Ok((result_ty, underlying_ability))
    }

    /// Infer the result type for `Async.race(ops)` where `ops: List<Ability<T, A!>>`.
    /// Returns `(T, A)` - the result type and the underlying ability to require.
    fn infer_async_race_type(
        &mut self,
        arg_tys: &[Type],
        span: (u32, u32),
    ) -> InferResult<(Type, AbilitySet)> {
        // Async.race takes exactly one argument
        if arg_tys.len() != 1 {
            return Err(type_error(
                TypeErrorKind::ArityMismatch {
                    expected: 1,
                    actual: arg_tys.len(),
                },
                span,
            ));
        }

        // Extract T and A from List<Ability<T, A!>>
        let (element_result_ty, underlying_ability) =
            self.extract_list_ability_types(&arg_tys[0], span)?;

        // Return T (just the element type, not wrapped in List)
        Ok((element_result_ty, underlying_ability))
    }

    /// Extract T and A from a type that should be `List<Ability<T, A!>>`.
    ///
    /// Returns the result type T and the ability set A.
    fn extract_list_ability_types(
        &mut self,
        ty: &Type,
        span: (u32, u32),
    ) -> InferResult<(Type, AbilitySet)> {
        let ty = self.apply(ty);

        // Create fresh type variables for T and A
        let expected_t = self.fresh();
        let expected_a = self.fresh_ability_var();
        let expected_ability_value = Type::ability_value(expected_t.clone(), expected_a.clone());
        let expected_list = Type::named("List", vec![expected_ability_value]);

        // Unify with the actual argument type
        self.unify(&ty, &expected_list, span)?;

        // Apply substitutions to get the concrete types
        let result_ty = self.apply(&expected_t);
        let ability_set = self.apply_abilities(&expected_a);

        Ok((result_ty, ability_set))
    }

    /// Instantiate a type scheme with fresh type variables.
    pub fn instantiate(&mut self, scheme: &Scheme) -> Type {
        if scheme.vars.is_empty() && scheme.ability_vars.is_empty() {
            return scheme.ty.clone();
        }

        let mut type_subst = HashMap::new();
        for var in &scheme.vars {
            type_subst.insert(*var, self.fresh());
        }

        let mut ability_subst = HashMap::new();
        for var in &scheme.ability_vars {
            ability_subst.insert(*var, self.fresh_ability_var());
        }

        scheme.ty.substitute_all(&type_subst, &ability_subst)
    }

    /// Generalize a type to a scheme by quantifying free variables
    /// not in the environment.
    #[must_use]
    pub fn generalize(&self, env: &TypeEnv, ty: &Type) -> Scheme {
        let ty = self.apply(ty);
        let ty_vars = ty.free_vars();
        let env_vars = env.free_vars();

        let free_type_vars: Vec<_> = ty_vars
            .into_iter()
            .filter(|v| !env_vars.contains(v))
            .collect();

        let ty_ability_vars = ty.free_ability_vars();
        let env_ability_vars = env.free_ability_vars();

        let free_ability_vars: Vec<_> = ty_ability_vars
            .into_iter()
            .filter(|v| !env_ability_vars.contains(v))
            .collect();

        if free_type_vars.is_empty() && free_ability_vars.is_empty() {
            Scheme::mono(ty)
        } else {
            Scheme::poly_with_abilities(free_type_vars, free_ability_vars, ty)
        }
    }

    /// Infer the type of an expression.
    ///
    /// # Errors
    ///
    /// Returns a `TypeError` if type inference fails.
    #[allow(clippy::too_many_lines)]
    pub fn infer_expr(&mut self, env: &TypeEnv, expr: &mut Expr) -> InferResult<Type> {
        let span = (expr.span.start, expr.span.end);
        let ty = match &mut expr.kind {
            ExprKind::Unit => Type::Unit,
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Number(_) => Type::Number,
            ExprKind::String(_) => Type::String,

            ExprKind::Local(id) => {
                let scheme = env
                    .get(*id)
                    .ok_or_else(|| type_error(TypeErrorKind::UndefinedBinding { id: *id }, span))?;
                self.instantiate(scheme)
            }

            ExprKind::Name(name) => {
                let scheme = env.get_by_name(&name.name).ok_or_else(|| {
                    type_error(
                        TypeErrorKind::UndefinedVariable {
                            name: name.name.clone(),
                        },
                        span,
                    )
                })?;
                self.instantiate(scheme)
            }

            ExprKind::Tuple(elems) => {
                let mut elem_tys = Vec::with_capacity(elems.len());
                for elem in elems {
                    elem_tys.push(self.infer_expr(env, elem)?);
                }
                Type::Tuple(elem_tys)
            }

            ExprKind::TupleIndex(tuple_expr, idx) => {
                let tuple_ty = self.infer_expr(env, tuple_expr)?;
                let tuple_ty = self.apply(&tuple_ty);
                match &tuple_ty {
                    Type::Tuple(elems) => {
                        let idx_usize = *idx as usize;
                        if idx_usize >= elems.len() {
                            return Err(type_error(
                                TypeErrorKind::TupleIndexOutOfBounds {
                                    index: *idx,
                                    tuple_ty: tuple_ty.clone(),
                                },
                                span,
                            ));
                        }
                        elems[idx_usize].clone()
                    }
                    Type::Var(_) => {
                        // Unknown tuple type, need more context
                        return Err(type_error(
                            TypeErrorKind::CannotInfer {
                                hint: "tuple type".to_string(),
                            },
                            span,
                        ));
                    }
                    _ => {
                        return Err(type_error(
                            TypeErrorKind::TypeMismatch {
                                expected: Type::Tuple(vec![]),
                                actual: tuple_ty,
                            },
                            span,
                        ));
                    }
                }
            }

            ExprKind::Record(fields) => {
                let mut field_tys = Vec::with_capacity(fields.len());
                for (name, expr) in fields {
                    let ty = self.infer_expr(env, expr)?;
                    field_tys.push((name.clone(), ty));
                }
                Type::record(field_tys)
            }

            ExprKind::RecordField(record_expr, field) => {
                let record_ty = self.infer_expr(env, record_expr)?;
                let record_ty = self.apply(&record_ty);
                match &record_ty {
                    Type::Record(rec) => rec.get_field(field).cloned().ok_or_else(|| {
                        TypeError::new(
                            TypeErrorKind::FieldNotFound {
                                field: field.clone(),
                                record_ty: record_ty.clone(),
                            },
                            span,
                        )
                    })?,
                    Type::Var(_) => {
                        return Err(type_error(
                            TypeErrorKind::CannotInfer {
                                hint: "record type".to_string(),
                            },
                            span,
                        ));
                    }
                    _ => {
                        return Err(type_error(
                            TypeErrorKind::FieldNotFound {
                                field: field.clone(),
                                record_ty: record_ty.clone(),
                            },
                            span,
                        ));
                    }
                }
            }

            ExprKind::List(elems) => {
                let elem_ty = self.fresh();
                for elem in elems {
                    let ty = self.infer_expr(env, elem)?;
                    self.unify(&elem_ty, &ty, span)?;
                }
                Type::named("List", vec![self.apply(&elem_ty)])
            }

            ExprKind::Binary(op, left, right) => {
                let left_ty = self.infer_expr(env, left)?;
                let right_ty = self.infer_expr(env, right)?;

                if op.is_arithmetic() {
                    self.unify(&left_ty, &Type::Number, span)?;
                    self.unify(&right_ty, &Type::Number, span)?;
                    Type::Number
                } else if op.is_comparison() {
                    // For now, require same types for comparison
                    self.unify(&left_ty, &right_ty, span)?;
                    Type::Bool
                } else if op.is_logical() {
                    self.unify(&left_ty, &Type::Bool, span)?;
                    self.unify(&right_ty, &Type::Bool, span)?;
                    Type::Bool
                } else {
                    unreachable!()
                }
            }

            ExprKind::Unary(op, operand) => {
                let operand_ty = self.infer_expr(env, operand)?;
                match op {
                    UnaryOp::Neg => {
                        self.unify(&operand_ty, &Type::Number, span)?;
                        Type::Number
                    }
                    UnaryOp::Not => {
                        self.unify(&operand_ty, &Type::Bool, span)?;
                        Type::Bool
                    }
                }
            }

            ExprKind::If(cond, then_branch, else_branch) => {
                let cond_ty = self.infer_expr(env, cond)?;
                self.unify(&cond_ty, &Type::Bool, span)?;

                let then_ty = self.infer_expr(env, then_branch)?;

                if let Some(else_branch) = else_branch {
                    let else_ty = self.infer_expr(env, else_branch)?;
                    self.unify(&then_ty, &else_ty, span)?;
                    then_ty
                } else {
                    // No else branch means unit type
                    self.unify(&then_ty, &Type::Unit, span)?;
                    Type::Unit
                }
            }

            ExprKind::Match(scrutinee, arms) => {
                let scrutinee_ty = self.infer_expr(env, scrutinee)?;

                if arms.is_empty() {
                    return Err(type_error(
                        TypeErrorKind::CannotInfer {
                            hint: "match expression has no arms".to_string(),
                        },
                        span,
                    ));
                }

                let result_ty = self.fresh();
                for arm in arms {
                    let arm_env = self.infer_pattern(env, &arm.pattern, &scrutinee_ty)?;
                    // Clone the arm body to avoid mutable borrow issues
                    let mut body = arm.body.clone();
                    let arm_ty = self.infer_expr(&arm_env, &mut body)?;
                    self.unify(&result_ty, &arm_ty, span)?;
                }
                self.apply(&result_ty)
            }

            ExprKind::Block(stmts, result) => {
                let mut block_env = env.extend();
                for stmt in stmts {
                    match &mut stmt.kind {
                        StmtKind::Let(binding) => {
                            let init_ty = self.infer_expr(&block_env, &mut binding.init)?;
                            let scheme = self.generalize(&block_env, &init_ty);
                            block_env.insert(binding.id, binding.name.clone(), scheme);
                        }
                        StmtKind::Expr(expr) => {
                            self.infer_expr(&block_env, expr)?;
                        }
                    }
                }
                if let Some(result) = result {
                    self.infer_expr(&block_env, result)?
                } else {
                    Type::Unit
                }
            }

            ExprKind::Lambda(lambda) => {
                let mut lambda_env = env.extend();
                let mut param_tys = Vec::with_capacity(lambda.params.len());

                for param in &lambda.params {
                    // Resolve holes in type annotations (e.g., `_` becomes a fresh variable)
                    let param_ty = match &param.ty {
                        Some(ty) => self.resolve_holes(ty),
                        None => self.fresh(),
                    };
                    param_tys.push(param_ty.clone());
                    lambda_env.insert_mono(param.id, param.name.clone(), param_ty);
                }

                let ret_ty = self.infer_expr(&lambda_env, &mut lambda.body)?;
                Type::function(param_tys, ret_ty)
            }

            ExprKind::Call(callee, args) => {
                // Check for intrinsic functions first
                if let ExprKind::Name(name) = &callee.kind {
                    if let Some(ret_ty) = self.try_infer_intrinsic(env, name, args, span)? {
                        return Ok(ret_ty);
                    }
                }

                let callee_ty = self.infer_expr(env, callee)?;
                let mut arg_tys = Vec::with_capacity(args.len());
                for arg in args {
                    arg_tys.push(self.infer_expr(env, arg)?);
                }

                let ret_ty = self.fresh();
                let expected_fn_ty = Type::function(arg_tys, ret_ty.clone());
                self.unify(&callee_ty, &expected_fn_ty, span)?;
                self.apply(&ret_ty)
            }

            ExprKind::Perform(ability_call) => {
                // Infer types of arguments
                let mut arg_tys = Vec::with_capacity(ability_call.args.len());
                for arg in &mut ability_call.args.clone() {
                    let mut arg_clone = arg.clone();
                    arg_tys.push(self.infer_expr(env, &mut arg_clone)?);
                }

                // Look up the ability and method to get return type and additional abilities
                let (ability_id, result_ty, additional_abilities) = self.lookup_ability_method(
                    &ability_call.ability.name,
                    &ability_call.method,
                    &arg_tys,
                    span,
                )?;

                // Add primary ability to current requirements
                self.require_ability(ability_id);

                // Add any additional abilities (e.g., underlying abilities from Async.all/race)
                self.require_abilities(&additional_abilities);

                result_ty
            }

            ExprKind::Suspend(ability_call) => {
                // Infer types of arguments
                let mut arg_tys = Vec::with_capacity(ability_call.args.len());
                for arg in &mut ability_call.args.clone() {
                    let mut arg_clone = arg.clone();
                    arg_tys.push(self.infer_expr(env, &mut arg_clone)?);
                }

                // Look up the ability and method to get return type
                // Note: For suspend, we ignore additional_abilities since we're just creating a value
                let (ability_id, result_ty, _additional_abilities) = self.lookup_ability_method(
                    &ability_call.ability.name,
                    &ability_call.method,
                    &arg_tys,
                    span,
                )?;

                // Return Ability<result_ty, ability_id> - a suspended ability value
                Type::ability_value(result_ty, AbilitySet::single(ability_id))
            }

            ExprKind::Handle(handle_expr) => {
                // Save current abilities
                let saved_abilities = self.current_abilities.clone();
                self.reset_abilities();

                // Infer the body expression
                let body_ty = self.infer_expr(env, &mut handle_expr.body.clone())?;

                // Get the abilities required by the body
                let body_abilities = self.current_abilities.clone();

                // Collect handled abilities from handler values (from `with` clause)
                let mut handled_abilities = Vec::new();
                for handler_value in &mut handle_expr.handler_values.clone() {
                    let handler_ty = self.infer_expr(env, &mut handler_value.clone())?;

                    // Handler value should be Handler<A>
                    if let Type::Handler(handler_type) = handler_ty {
                        handled_abilities.push(handler_type.ability);
                    } else {
                        return Err(type_error(
                            TypeErrorKind::TypeMismatch {
                                expected: Type::handler(0), // Generic Handler type
                                actual: handler_ty,
                            },
                            (handler_value.span.start, handler_value.span.end),
                        ));
                    }
                }

                // Collect handled abilities from inline handlers
                for handler in &handle_expr.handlers {
                    if let Some(ability_id) = self.ability_name_to_id(&handler.ability.name) {
                        handled_abilities.push(ability_id);
                    }

                    // Infer handler body (with continuation parameter)
                    let mut handler_env = env.extend();
                    for param in &handler.params {
                        let param_ty = param.ty.clone().unwrap_or_else(|| self.fresh());
                        handler_env.insert_mono(param.id, param.name.clone(), param_ty);
                    }

                    // Handler body should produce the same type as the overall handle expression
                    // (when resume is called, it continues with the body's result type)
                    let mut handler_body = handler.body.clone();
                    let handler_ty = self.infer_expr(&handler_env, &mut handler_body)?;
                    self.unify(&body_ty, &handler_ty, span)?;
                }

                // Compute remaining unhandled abilities
                let remaining_abilities = match &body_abilities {
                    AbilitySet::Empty => AbilitySet::Empty,
                    AbilitySet::Concrete(abilities) => {
                        let remaining: Vec<_> = abilities
                            .iter()
                            .filter(|a| !handled_abilities.contains(a))
                            .copied()
                            .collect();
                        AbilitySet::from_abilities(remaining)
                    }
                    AbilitySet::Var(_) | AbilitySet::Row { .. } => {
                        // Can't statically determine which abilities are handled
                        // For now, assume all handlers match
                        body_abilities.clone()
                    }
                };

                // Restore saved abilities and add remaining
                self.current_abilities = saved_abilities.union(&remaining_abilities);

                // Handle else clause if present
                if let Some(else_clause) = &mut handle_expr.else_clause.clone() {
                    let else_ty = self.infer_expr(env, else_clause)?;
                    self.unify(&body_ty, &else_ty, span)?;
                }

                body_ty
            }

            ExprKind::Resume(value) => {
                // Resume transfers control to a continuation.
                // The value type should match what the continuation expects.
                // For now, just type-check the value and return a fresh type variable,
                // since resume doesn't return normally.
                let _value_ty = self.infer_expr(env, value)?;

                // Resume doesn't return normally, so we use a fresh type variable.
                // In a more complete implementation, this would be a never type (!).
                self.fresh()
            }

            ExprKind::HandlerLiteral(handler_lit) => {
                // Handler literal type checking (Milestone 13)
                //
                // 1. Collect method names from the handler literal
                // 2. Infer which ability this handler is for (from method names)
                // 3. Verify each method matches the ability's signature
                // 4. Type-check each handler body in an appropriate environment
                // 5. Return Handler<A> type

                // Collect method names
                let method_names: Vec<Arc<str>> = handler_lit
                    .methods
                    .iter()
                    .map(|m| m.method.clone())
                    .collect();

                // Try to infer the target ability from method names
                let ability_id =
                    self.infer_ability_from_methods(&method_names)
                        .ok_or_else(|| {
                            TypeError::new(
                                TypeErrorKind::HandlerAbilityAmbiguous {
                                    method_names: method_names.clone(),
                                },
                                span,
                            )
                        })?;

                let ability_name: Arc<str> = self
                    .ability_id_to_name(ability_id)
                    .unwrap_or("unknown")
                    .into();

                // Get the ability's method signatures
                let ability_signatures = self.get_ability_method_signatures(ability_id);

                // Verify each method in the handler matches the ability
                for method in &mut handler_lit.methods {
                    // Find the corresponding ability method signature
                    let sig = ability_signatures
                        .iter()
                        .find(|(name, _, _)| name == method.method.as_ref());

                    if let Some((_, expected_param_count, expected_return_ty)) = sig {
                        // Check arity (excluding implicit continuation parameter)
                        if method.params.len() != *expected_param_count {
                            return Err(type_error(
                                TypeErrorKind::HandlerMethodArityMismatch {
                                    ability: ability_name.clone(),
                                    method: method.method.clone(),
                                    expected: *expected_param_count,
                                    actual: method.params.len(),
                                },
                                (method.span.start, method.span.end),
                            ));
                        }

                        // Type-check the handler body with method parameters in scope
                        let mut method_env = env.extend();
                        for param in &method.params {
                            // Parameter types are inferred (fresh type variables)
                            let param_ty = param.ty.clone().unwrap_or_else(|| self.fresh());
                            method_env.insert_mono(param.id, param.name.clone(), param_ty);
                        }

                        // The handler body type should be compatible with resume behavior
                        // For most methods, the body should eventually call resume()
                        // The resume value type determines what's returned to the call site
                        let body_ty = self.infer_expr(&method_env, &mut method.body)?;

                        // If we have a concrete expected return type, try to unify
                        // (Hole means polymorphic - we don't constrain)
                        if *expected_return_ty != Type::Hole && *expected_return_ty != Type::Never {
                            // Note: The body type isn't directly the return type -
                            // the return type is what resume() is called with.
                            // For now, we just type-check the body without strict constraints.
                            let _ = body_ty; // Body is checked but not constrained
                        }
                    } else {
                        // Method not found in ability
                        return Err(type_error(
                            TypeErrorKind::HandlerUnknownMethod {
                                ability: ability_name.clone(),
                                method: method.method.clone(),
                            },
                            (method.span.start, method.span.end),
                        ));
                    }
                }

                // Handlers don't need to provide all methods - partial handlers are allowed
                // (they can be composed with other handlers that provide missing methods)

                Type::handler(ability_id)
            }

            ExprKind::Sandbox(sandbox_expr) => {
                // Sandbox type checking (Milestone 14)
                //
                // A sandbox restricts the abilities available within its body to only
                // those explicitly allowed. This enables running untrusted code with
                // limited capabilities.
                //
                // 1. Save the current ability context
                // 2. Create a new ability context with only allowed abilities
                // 3. Type-check the body in this restricted context
                // 4. Verify the body only uses allowed abilities
                // 5. Restore the original ability context

                // Save current abilities
                let saved_abilities = self.current_abilities.clone();

                // Reset to empty abilities - only allowed abilities will be available
                self.reset_abilities();

                // Convert allowed ability names to IDs
                let allowed_ability_ids: Vec<AbilityId> = sandbox_expr
                    .allowed_abilities
                    .iter()
                    .filter_map(|name| self.ability_name_to_id(&name.name))
                    .collect();

                // Check for unknown abilities
                for ability_name in &sandbox_expr.allowed_abilities {
                    if self.ability_name_to_id(&ability_name.name).is_none() {
                        return Err(type_error(
                            TypeErrorKind::UnknownAbility {
                                name: ability_name.name.clone(),
                            },
                            span,
                        ));
                    }
                }

                // Infer the body expression
                let body_ty = self.infer_expr(env, &mut sandbox_expr.body.clone())?;

                // Get the abilities required by the body
                let body_abilities = self.current_abilities.clone();

                // Verify that the body only uses allowed abilities
                if let AbilitySet::Concrete(required_abilities) = &body_abilities {
                    // Check each required ability is in the allowed list
                    for ability_id in required_abilities {
                        if !allowed_ability_ids.contains(ability_id) {
                            let ability_name =
                                self.ability_id_to_name(*ability_id).unwrap_or("unknown");
                            return Err(type_error(
                                TypeErrorKind::SandboxAbilityViolation {
                                    ability: ability_name.into(),
                                    allowed: sandbox_expr
                                        .allowed_abilities
                                        .iter()
                                        .map(|n| n.name.clone())
                                        .collect(),
                                },
                                span,
                            ));
                        }
                    }
                }
                // Note: AbilitySet::Empty (pure), Var, and Row are allowed
                // Polymorphic abilities can't be statically checked

                // Restore saved abilities (sandbox doesn't add any abilities to the outer context)
                self.current_abilities = saved_abilities;

                body_ty
            }
        };

        expr.ty = Some(ty.clone());
        Ok(ty)
    }

    /// Infer types for a pattern and return extended environment.
    fn infer_pattern(
        &mut self,
        env: &TypeEnv,
        pattern: &Pattern,
        expected: &Type,
    ) -> InferResult<TypeEnv> {
        let span = (pattern.span.start, pattern.span.end);
        let mut new_env = env.extend();

        match &pattern.kind {
            PatternKind::Wildcard | PatternKind::Variant(_, _) => {
                // Wildcard matches anything
                // Variant patterns require enum type definitions (future work)
            }

            PatternKind::Binding(id, name) => {
                new_env.insert_mono(*id, name.clone(), expected.clone());
            }

            PatternKind::Literal(lit) => {
                let lit_ty = match lit {
                    crate::ast::Literal::Unit => Type::Unit,
                    crate::ast::Literal::Bool(_) => Type::Bool,
                    crate::ast::Literal::Number(_) => Type::Number,
                    crate::ast::Literal::String(_) => Type::String,
                };
                self.unify(expected, &lit_ty, span)?;
            }

            PatternKind::Tuple(patterns) => {
                let elem_tys: Vec<_> = (0..patterns.len()).map(|_| self.fresh()).collect();
                let tuple_ty = Type::Tuple(elem_tys.clone());
                self.unify(expected, &tuple_ty, span)?;

                for (pat, ty) in patterns.iter().zip(elem_tys.iter()) {
                    let pat_env = self.infer_pattern(&new_env, pat, ty)?;
                    // Merge bindings
                    for (id, name, scheme) in pat_env.iter_named() {
                        new_env.insert(id, name.clone(), scheme.clone());
                    }
                }
            }

            PatternKind::Record(field_patterns) => {
                let mut field_tys = Vec::with_capacity(field_patterns.len());
                for (name, _) in field_patterns {
                    field_tys.push((name.clone(), self.fresh()));
                }
                let record_ty = Type::record(field_tys.clone());
                self.unify(expected, &record_ty, span)?;

                for ((_, pat), (_, ty)) in field_patterns.iter().zip(field_tys.iter()) {
                    let pat_env = self.infer_pattern(&new_env, pat, ty)?;
                    for (id, name, scheme) in pat_env.iter_named() {
                        new_env.insert(id, name.clone(), scheme.clone());
                    }
                }
            }
        }

        Ok(new_env)
    }
}

#[cfg(test)]
mod tests;

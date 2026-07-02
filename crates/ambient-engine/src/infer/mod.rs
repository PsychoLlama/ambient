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
//! - [`unify`] - Type and ability unification
//! - [`expr`] - Expression type inference
//! - [`pattern`] - Pattern matching inference
//! - [`intrinsics`] - Intrinsic function type inference
//! - [`abilities`] - Ability lookup and async type inference
//! - [`Infer`] - The main type inference engine

mod abilities;
mod check;
mod effects;
mod env;
mod error;
mod expr;
mod intrinsics;
mod pattern;
mod unify;

pub use check::{
    check_module, check_module_with_registry, check_module_with_registry_and_resolver, CheckResult,
};
pub use env::{Scheme, TypeEnv};
pub use error::{BoxedTypeError, BoxedTypeErrorExt, InferResult, TypeError, TypeErrorKind};

use error::type_error;

use std::collections::HashMap;
use std::sync::Arc;

use crate::ability_resolver::AbilityResolver;
use crate::types::{
    AbilityId, AbilityRegistry, AbilitySet, AbilityValueType, AbilityVarId, ForallType, NamedType,
    NominalType, RecordType, TraitRegistry, Type, TypeVarGen, TypeVarId,
};

// ─────────────────────────────────────────────────────────────────────────────
// Type Inference
// ─────────────────────────────────────────────────────────────────────────────

/// Type inference context.
pub struct Infer {
    /// Type variable generator.
    gen: TypeVarGen,
    /// Substitution mapping type variables to their bindings.
    pub(crate) subst: HashMap<TypeVarId, Type>,
    /// Substitution mapping ability variables to their bindings (Milestone 8).
    pub(crate) ability_subst: HashMap<AbilityVarId, AbilitySet>,
    /// Current ability requirements being accumulated (Milestone 8).
    /// This tracks abilities used in the current function being inferred.
    pub(crate) current_abilities: AbilitySet,
    /// Optional ability registry for dependency tracking.
    pub(crate) ability_registry: Option<AbilityRegistry>,
    /// Ability resolver for looking up ability and method information.
    pub(crate) ability_resolver: AbilityResolver,
    /// Type alias registry for looking up types by name.
    /// Maps type alias names to their resolved types (including Nominal types).
    pub(crate) type_aliases: HashMap<Arc<str>, Type>,
    /// Trait registry for trait and impl lookup.
    pub(crate) trait_registry: TraitRegistry,
    /// Errors recorded outside the normal `InferResult` flow (e.g. unknown
    /// ability names found while resolving annotations). Drained by the
    /// module-level check functions.
    pub(crate) pending_errors: Vec<error::BoxedTypeError>,
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
            type_aliases: HashMap::new(),
            trait_registry: TraitRegistry::new(),
            pending_errors: Vec::new(),
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
            type_aliases: HashMap::new(),
            trait_registry: TraitRegistry::new(),
            pending_errors: Vec::new(),
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
            type_aliases: HashMap::new(),
            trait_registry: TraitRegistry::new(),
            pending_errors: Vec::new(),
        }
    }

    /// Register a type alias for later lookup during typed record inference.
    pub fn register_type_alias(&mut self, name: Arc<str>, ty: Type) {
        self.type_aliases.insert(name, ty);
    }

    /// Look up a type alias by name.
    #[must_use]
    pub fn get_type_alias(&self, name: &str) -> Option<&Type> {
        self.type_aliases.get(name)
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
                let abilities = self.resolve_ability_annotation(&f.abilities);
                Type::function_with_abilities(params, ret, abilities)
            }
            Type::Named(n) => {
                // Check if this named type corresponds to a registered type alias
                if n.args.is_empty() {
                    if let Some(aliased_type) = self.type_aliases.get(&n.name).cloned() {
                        // Resolve to the aliased type
                        return self.resolve_holes(&aliased_type);
                    }
                }
                // Otherwise keep as named type with resolved args
                Type::Named(NamedType::new(
                    n.name.clone(),
                    n.args.iter().map(|a| self.resolve_holes(a)).collect(),
                ))
            }
            Type::Nominal(n) => Type::Nominal(NominalType::new(
                n.uuid,
                self.resolve_holes(&n.inner),
                n.name.clone(),
            )),
            Type::AbilityValue(av) => Type::AbilityValue(AbilityValueType::new(
                self.resolve_holes(&av.result),
                self.resolve_ability_annotation(&av.ability),
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

    /// Resolve ability names from a source annotation to concrete ability IDs.
    ///
    /// Lowering has no ability resolver, so annotations like
    /// `(T) -> U with Console` arrive as `AbilitySet::Unresolved(["Console"])`.
    /// Unknown names are recorded in `pending_errors` (drained by the
    /// module-level check functions) rather than silently dropped.
    fn resolve_ability_annotation(&mut self, abilities: &AbilitySet) -> AbilitySet {
        let AbilitySet::Unresolved(names) = abilities else {
            return abilities.clone();
        };

        let mut ids = Vec::new();
        for name in names {
            if let Some(id) = self.ability_name_to_id(name) {
                ids.push(id);
            } else {
                self.pending_errors.push(Box::new(TypeError::new(
                    TypeErrorKind::UnknownAbility {
                        name: Arc::clone(name),
                    },
                    (0, 0),
                )));
            }
        }
        AbilitySet::from_abilities(ids)
    }

    /// Take any errors recorded outside the normal `InferResult` flow.
    pub(crate) fn take_pending_errors(&mut self) -> Vec<error::BoxedTypeError> {
        std::mem::take(&mut self.pending_errors)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_error_display() {
        let err = TypeError::new(
            TypeErrorKind::TypeMismatch {
                expected: Type::Number,
                actual: Type::String,
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("type mismatch"));
        assert!(msg.contains("number"));
        assert!(msg.contains("string"));
    }

    #[test]
    fn test_ability_error_display() {
        let err = TypeError::new(
            TypeErrorKind::AbilityMismatch {
                expected: AbilitySet::from_abilities([1]),
                actual: AbilitySet::from_abilities([2]),
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("ability mismatch"));

        let err2 = TypeError::new(
            TypeErrorKind::UnknownAbility { name: "Foo".into() },
            (0, 10),
        );
        let msg2 = format!("{err2}");
        assert!(msg2.contains("unknown ability"));
        assert!(msg2.contains("Foo"));
    }

    #[test]
    fn test_error_display_field_not_found() {
        let err = TypeError::new(
            TypeErrorKind::FieldNotFound {
                field: "missing".into(),
                record_ty: Type::record([("x", Type::Number)]),
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("missing") || msg.contains("field"));
    }

    #[test]
    fn test_error_display_tuple_index_out_of_bounds() {
        let err = TypeError::new(
            TypeErrorKind::TupleIndexOutOfBounds {
                index: 5,
                tuple_ty: Type::Tuple(vec![Type::Number, Type::String]),
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("5") || msg.contains("out of bounds") || msg.contains("index"));
    }

    #[test]
    fn test_error_display_not_a_function() {
        let err = TypeError::new(TypeErrorKind::NotAFunction { ty: Type::Number }, (0, 10));
        let msg = format!("{err}");
        assert!(msg.contains("not a function") || msg.contains("number"));
    }

    #[test]
    fn test_error_display_non_boolean_condition() {
        let err = TypeError::new(
            TypeErrorKind::NonBooleanCondition { ty: Type::Number },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("condition") || msg.contains("bool"));
    }

    #[test]
    fn test_error_display_arity_mismatch() {
        let err = TypeError::new(
            TypeErrorKind::ArityMismatch {
                expected: 2,
                actual: 1,
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("2") && msg.contains("1"));
    }

    #[test]
    fn test_error_display_match_arm_type_mismatch() {
        let err = TypeError::new(
            TypeErrorKind::MatchArmTypeMismatch {
                first: Type::Number,
                arm: Type::String,
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("match") || msg.contains("arm"));
    }

    #[test]
    fn test_error_display_undefined_variable() {
        let err = TypeError::new(
            TypeErrorKind::UndefinedVariable { name: "foo".into() },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("foo") || msg.contains("undefined"));
    }

    #[test]
    fn test_error_display_missing_ability() {
        let err = TypeError::new(
            TypeErrorKind::MissingAbility {
                required: 1,
                available: AbilitySet::Empty,
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("ability") || msg.contains("missing") || msg.contains("require"));
    }

    #[test]
    fn test_error_display_sandbox_ability_violation() {
        let err = TypeError::new(
            TypeErrorKind::SandboxAbilityViolation {
                ability: "FileSystem".into(),
                allowed: vec!["Console".into()],
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("sandbox") || msg.contains("FileSystem") || msg.contains("not allowed")
        );
    }

    #[test]
    fn test_error_display_handler_missing_method() {
        let err = TypeError::new(
            TypeErrorKind::HandlerMissingMethod {
                ability: "Console".into(),
                method: "print".into(),
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("print") || msg.contains("missing") || msg.contains("Console"));
    }

    #[test]
    fn test_error_display_infinite_type() {
        let err = TypeError::new(
            TypeErrorKind::InfiniteType {
                var: 0,
                ty: Type::function(vec![Type::var(0)], Type::var(0)),
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("infinite") || msg.contains("recursive") || msg.contains("occurs"));
    }

    #[test]
    fn test_error_display_cannot_infer() {
        let err = TypeError::new(
            TypeErrorKind::CannotInfer {
                hint: "ambiguous record field access".into(),
            },
            (0, 10),
        );
        let msg = format!("{err}");
        assert!(msg.contains("cannot") || msg.contains("infer") || msg.contains("ambiguous"));
    }
}

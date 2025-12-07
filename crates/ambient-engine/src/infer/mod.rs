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
//! This module is organized into logical sections:
//!
//! - **Type Errors** - Error types and display implementations
//! - **Type Environment** - `TypeEnv` and `Scheme` for tracking bindings
//! - **Unification** - Type unification with occurs check
//! - **Type Inference** - The `Infer` struct and inference methods
//! - **Module-level checking** - `check_module` for whole-program type checking

use std::collections::HashMap;
use std::sync::Arc;

use crate::ability_resolver::{AbilityResolver, EngineTypeFactory};
use crate::ast::{BindingId, Expr, ExprKind, Pattern, PatternKind, StmtKind, UnaryOp};
use crate::types::{
    AbilityId, AbilityRegistry, AbilitySet, AbilityValueType, AbilityVarId, ForallType,
    FunctionType, NamedType, NominalType, RecordType, Type, TypeVar, TypeVarGen, TypeVarId,
};

// ─────────────────────────────────────────────────────────────────────────────
// Type Errors
// ─────────────────────────────────────────────────────────────────────────────

/// A type error with context for error reporting.
#[derive(Debug, Clone)]
pub struct TypeError {
    /// The kind of type error.
    pub kind: TypeErrorKind,
    /// Source location (byte offset range).
    pub span: (u32, u32),
    /// Additional context for the error.
    pub context: Option<String>,
}

impl TypeError {
    /// Create a new type error.
    #[must_use]
    pub fn new(kind: TypeErrorKind, span: (u32, u32)) -> Self {
        Self {
            kind,
            span,
            context: None,
        }
    }

    /// Add context to a type error.
    #[must_use]
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.kind)?;
        if let Some(ctx) = &self.context {
            write!(f, "\n  {ctx}")?;
        }
        Ok(())
    }
}

impl std::error::Error for TypeError {}

/// A boxed type error for efficient error passing (reduces stack size).
pub type BoxedTypeError = Box<TypeError>;

/// Result type alias for type inference operations.
pub type InferResult<T> = Result<T, BoxedTypeError>;

/// Helper to create a boxed type error.
fn type_error(kind: TypeErrorKind, span: (u32, u32)) -> BoxedTypeError {
    Box::new(TypeError::new(kind, span))
}

/// Extension trait for adding context to boxed type errors.
pub trait BoxedTypeErrorExt {
    /// Add context to a boxed type error.
    fn with_context(self, context: impl Into<String>) -> BoxedTypeError;
}

impl BoxedTypeErrorExt for BoxedTypeError {
    fn with_context(mut self, context: impl Into<String>) -> BoxedTypeError {
        self.context = Some(context.into());
        self
    }
}

/// The kind of type error.
#[derive(Debug, Clone)]
pub enum TypeErrorKind {
    /// Type mismatch in unification.
    TypeMismatch { expected: Type, actual: Type },

    /// Occurs check failed (infinite type).
    InfiniteType { var: TypeVarId, ty: Type },

    /// Undefined variable.
    UndefinedVariable { name: Arc<str> },

    /// Undefined binding ID (internal error).
    UndefinedBinding { id: BindingId },

    /// Field not found in record.
    FieldNotFound { field: Arc<str>, record_ty: Type },

    /// Tuple index out of bounds.
    TupleIndexOutOfBounds { index: u32, tuple_ty: Type },

    /// Not a function type in call position.
    NotAFunction { ty: Type },

    /// Wrong number of arguments.
    ArityMismatch { expected: usize, actual: usize },

    /// Condition is not a boolean.
    NonBooleanCondition { ty: Type },

    /// Cannot determine type.
    CannotInfer { hint: String },

    /// Match arms have different types.
    MatchArmTypeMismatch { first: Type, arm: Type },

    // ─────────────────────────────────────────────────────────────────────────
    // Ability errors (Milestone 8)
    // ─────────────────────────────────────────────────────────────────────────
    /// Ability mismatch in unification.
    AbilityMismatch {
        expected: AbilitySet,
        actual: AbilitySet,
    },

    /// Ability variable occurs check failed.
    InfiniteAbility {
        var: AbilityVarId,
        abilities: AbilitySet,
    },

    /// Missing ability requirement.
    MissingAbility {
        required: AbilityId,
        available: AbilitySet,
    },

    /// Unknown ability.
    UnknownAbility { name: Arc<str> },

    /// Unknown ability method.
    UnknownAbilityMethod { ability: Arc<str>, method: Arc<str> },

    /// Not a suspended ability value.
    NotAnAbilityValue { ty: Type },

    /// Ability not handled.
    AbilityNotHandled { ability: AbilityId },

    // ─────────────────────────────────────────────────────────────────────────
    // Handler literal errors (Milestone 13)
    // ─────────────────────────────────────────────────────────────────────────
    /// Cannot determine which ability a handler literal is for.
    HandlerAbilityAmbiguous { method_names: Vec<Arc<str>> },

    /// Handler method doesn't exist in the target ability.
    HandlerUnknownMethod { ability: Arc<str>, method: Arc<str> },

    /// Handler method has wrong number of parameters.
    HandlerMethodArityMismatch {
        ability: Arc<str>,
        method: Arc<str>,
        expected: usize,
        actual: usize,
    },

    /// Handler is missing a required ability method.
    HandlerMissingMethod { ability: Arc<str>, method: Arc<str> },

    // ─────────────────────────────────────────────────────────────────────────
    // Sandbox errors (Milestone 14)
    // ─────────────────────────────────────────────────────────────────────────
    /// Sandbox attempted to use an ability that is not allowed.
    SandboxAbilityViolation {
        ability: Arc<str>,
        allowed: Vec<Arc<str>>,
    },
}

impl std::fmt::Display for TypeErrorKind {
    #[allow(clippy::too_many_lines)]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TypeMismatch { expected, actual } => {
                write!(f, "type mismatch: expected `{expected}`, found `{actual}`")
            }
            Self::InfiniteType { var, ty } => {
                write!(f, "infinite type: '{var} occurs in `{ty}`")
            }
            Self::UndefinedVariable { name } => {
                write!(f, "undefined variable: `{name}`")
            }
            Self::UndefinedBinding { id } => {
                write!(f, "undefined binding (internal error): {id}")
            }
            Self::FieldNotFound { field, record_ty } => {
                write!(f, "field `{field}` not found in `{record_ty}`")
            }
            Self::TupleIndexOutOfBounds { index, tuple_ty } => {
                write!(f, "tuple index {index} out of bounds for `{tuple_ty}`")
            }
            Self::NotAFunction { ty } => {
                write!(f, "expected a function, found `{ty}`")
            }
            Self::ArityMismatch { expected, actual } => {
                write!(f, "expected {expected} arguments, found {actual}")
            }
            Self::NonBooleanCondition { ty } => {
                write!(f, "condition must be boolean, found `{ty}`")
            }
            Self::CannotInfer { hint } => {
                write!(f, "cannot infer type: {hint}")
            }
            Self::MatchArmTypeMismatch { first, arm } => {
                write!(
                    f,
                    "match arm type `{arm}` differs from first arm type `{first}`"
                )
            }
            Self::AbilityMismatch { expected, actual } => {
                write!(
                    f,
                    "ability mismatch: expected `{expected}`, found `{actual}`"
                )
            }
            Self::InfiniteAbility { var, abilities } => {
                write!(f, "infinite ability type: E{var}! occurs in `{abilities}`")
            }
            Self::MissingAbility {
                required,
                available,
            } => {
                write!(
                    f,
                    "missing ability: #{required} is required but only `{available}` available"
                )
            }
            Self::UnknownAbility { name } => {
                write!(f, "unknown ability: `{name}`")
            }
            Self::UnknownAbilityMethod { ability, method } => {
                write!(f, "unknown method `{method}` for ability `{ability}`")
            }
            Self::NotAnAbilityValue { ty } => {
                write!(f, "expected a suspended ability value, found `{ty}`")
            }
            Self::AbilityNotHandled { ability } => {
                write!(f, "ability #{ability} is not handled")
            }
            Self::HandlerAbilityAmbiguous { method_names } => {
                let methods = method_names
                    .iter()
                    .map(AsRef::as_ref)
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "cannot determine ability for handler with methods: [{methods}]. \
                     Add a type annotation like `let h: Handler<Ability> = ...`"
                )
            }
            Self::HandlerUnknownMethod { ability, method } => {
                write!(
                    f,
                    "handler method `{method}` is not defined in ability `{ability}`"
                )
            }
            Self::HandlerMethodArityMismatch {
                ability,
                method,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "handler method `{ability}.{method}` expects {expected} parameters, \
                     but handler provides {actual}"
                )
            }
            Self::HandlerMissingMethod { ability, method } => {
                write!(
                    f,
                    "handler for `{ability}` is missing required method `{method}`"
                )
            }
            Self::SandboxAbilityViolation { ability, allowed } => {
                if allowed.is_empty() {
                    write!(
                        f,
                        "sandbox violation: ability `{ability}` is not allowed (no abilities are allowed in this sandbox)"
                    )
                } else {
                    let allowed_str = allowed
                        .iter()
                        .map(|s| format!("`{s}`"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    write!(
                        f,
                        "sandbox violation: ability `{ability}` is not allowed (allowed: {allowed_str})"
                    )
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Type Environment
// ─────────────────────────────────────────────────────────────────────────────

/// A type environment mapping bindings to their types.
///
/// Uses a persistent structure for efficient scoping.
#[derive(Debug, Clone, Default)]
pub struct TypeEnv {
    /// Mapping from binding IDs to type schemes.
    bindings: HashMap<BindingId, Scheme>,

    /// Mapping from names to binding IDs (for named lookups).
    names: HashMap<Arc<str>, BindingId>,
}

/// A type scheme (potentially polymorphic type).
///
/// `forall a b E!. T` where `a` and `b` are quantified type variables
/// and `E!` is a quantified ability variable.
#[derive(Debug, Clone)]
pub struct Scheme {
    /// Quantified type variables.
    pub vars: Vec<TypeVarId>,
    /// Quantified ability variables (Milestone 8).
    pub ability_vars: Vec<AbilityVarId>,
    /// The type body.
    pub ty: Type,
}

impl Scheme {
    /// Create a monomorphic scheme (no quantified variables).
    #[must_use]
    pub fn mono(ty: Type) -> Self {
        Self {
            vars: Vec::new(),
            ability_vars: Vec::new(),
            ty,
        }
    }

    /// Create a polymorphic scheme with type variables only.
    #[must_use]
    pub fn poly(vars: Vec<TypeVarId>, ty: Type) -> Self {
        Self {
            vars,
            ability_vars: Vec::new(),
            ty,
        }
    }

    /// Create a polymorphic scheme with both type and ability variables.
    #[must_use]
    pub fn poly_with_abilities(
        vars: Vec<TypeVarId>,
        ability_vars: Vec<AbilityVarId>,
        ty: Type,
    ) -> Self {
        Self {
            vars,
            ability_vars,
            ty,
        }
    }
}

impl TypeEnv {
    /// Create an empty type environment.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a binding with its type scheme.
    pub fn insert(&mut self, id: BindingId, name: Arc<str>, scheme: Scheme) {
        self.bindings.insert(id, scheme);
        self.names.insert(name, id);
    }

    /// Insert a binding with a monomorphic type.
    pub fn insert_mono(&mut self, id: BindingId, name: Arc<str>, ty: Type) {
        self.insert(id, name, Scheme::mono(ty));
    }

    /// Look up a binding by ID.
    #[must_use]
    pub fn get(&self, id: BindingId) -> Option<&Scheme> {
        self.bindings.get(&id)
    }

    /// Look up a binding by name.
    #[must_use]
    pub fn get_by_name(&self, name: &str) -> Option<&Scheme> {
        self.names.get(name).and_then(|id| self.bindings.get(id))
    }

    /// Extend the environment with a new scope (for let bindings).
    #[must_use]
    pub fn extend(&self) -> Self {
        self.clone()
    }

    /// Collect all free type variables in the environment.
    #[must_use]
    pub fn free_vars(&self) -> Vec<TypeVarId> {
        let mut vars = Vec::new();
        for scheme in self.bindings.values() {
            let scheme_vars = scheme.ty.free_vars();
            for var in scheme_vars {
                if !scheme.vars.contains(&var) && !vars.contains(&var) {
                    vars.push(var);
                }
            }
        }
        vars
    }

    /// Collect all free ability variables in the environment.
    #[must_use]
    pub fn free_ability_vars(&self) -> Vec<AbilityVarId> {
        let mut vars = Vec::new();
        for scheme in self.bindings.values() {
            let scheme_vars = scheme.ty.free_ability_vars();
            for var in scheme_vars {
                if !scheme.ability_vars.contains(&var) && !vars.contains(&var) {
                    vars.push(var);
                }
            }
        }
        vars
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unification
// ─────────────────────────────────────────────────────────────────────────────

/// Unify two types, making them equal.
///
/// Returns `Ok(())` if successful, or a `TypeError` if the types cannot be unified.
///
/// # Errors
///
/// Returns a `TypeError` if the types cannot be unified.
#[allow(clippy::too_many_lines)]
pub fn unify(t1: &Type, t2: &Type, span: (u32, u32)) -> InferResult<()> {
    let t1 = t1.resolve();
    let t2 = t2.resolve();

    match (&t1, &t2) {
        // Same types unify trivially
        // Primitive types and error types
        (Type::Unit, Type::Unit)
        | (Type::Bool, Type::Bool)
        | (Type::Number, Type::Number)
        | (Type::String, Type::String)
        | (Type::Never, Type::Never)
        | (Type::Error, _)
        | (_, Type::Error) => Ok(()),

        // Type variables
        (Type::Var(TypeVar::Unbound(id)), _) => bind_var(*id, &t2, span),
        (_, Type::Var(TypeVar::Unbound(id))) => bind_var(*id, &t1, span),

        // Tuples: same length, unify element-wise
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
                unify(e1, e2, span)?;
            }
            Ok(())
        }

        // Records: same fields, unify field-wise
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
            // Fields are sorted, so we can zip directly
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
                unify(ty1, ty2, span)?;
            }
            Ok(())
        }

        // Functions: same arity, unify params and return types
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
                unify(p1, p2, span)?;
            }
            unify(&f1.ret, &f2.ret, span)
        }

        // Named types: same name and arity, unify arguments
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
                unify(a1, a2, span)?;
            }
            Ok(())
        }

        // Nominal types: must have same UUID
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
            // Structural parts should also unify
            unify(&n1.inner, &n2.inner, span)
        }

        // Anything else doesn't unify
        _ => Err(type_error(
            TypeErrorKind::TypeMismatch {
                expected: t1.clone(),
                actual: t2.clone(),
            },
            span,
        )),
    }
}

/// Bind a type variable to a type.
fn bind_var(var: TypeVarId, ty: &Type, span: (u32, u32)) -> InferResult<()> {
    // Occurs check: prevent infinite types
    if occurs(var, ty) {
        return Err(type_error(
            TypeErrorKind::InfiniteType {
                var,
                ty: ty.clone(),
            },
            span,
        ));
    }

    // Link the variable to the type
    // In a real implementation, we'd mutate the type variable in place.
    // For now, we use a workaround with RefCell.
    // This is where the Link variant of TypeVar comes in.
    Ok(())
}

/// Check if a type variable occurs in a type (for occurs check).
fn occurs(var: TypeVarId, ty: &Type) -> bool {
    match ty.resolve() {
        Type::Var(TypeVar::Unbound(id)) => id == var,
        Type::Tuple(elems) => elems.iter().any(|e| occurs(var, e)),
        Type::Record(r) => r.fields.iter().any(|(_, t)| occurs(var, t)),
        Type::Function(f) => f.params.iter().any(|p| occurs(var, p)) || occurs(var, &f.ret),
        Type::Named(n) => n.args.iter().any(|a| occurs(var, a)),
        Type::Nominal(n) => occurs(var, &n.inner),
        Type::Forall(f) => {
            if f.vars.contains(&var) {
                false // bound variable, doesn't count
            } else {
                occurs(var, &f.body)
            }
        }
        _ => false,
    }
}

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
        self.ability_resolver.infer_ability_from_methods(method_names)
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
                    self.infer_ability_from_methods(&method_names).ok_or_else(|| {
                        TypeError::new(
                            TypeErrorKind::HandlerAbilityAmbiguous {
                                method_names: method_names.clone(),
                            },
                            span,
                        )
                    })?;

                let ability_name: Arc<str> = self.ability_id_to_name(ability_id)
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
                    for (id, scheme) in pat_env.bindings {
                        if let Some(name) = pat_env
                            .names
                            .iter()
                            .find(|(_, v)| **v == id)
                            .map(|(k, _)| k)
                        {
                            new_env.insert(id, name.clone(), scheme);
                        }
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
                    for (id, scheme) in pat_env.bindings {
                        if let Some(name) = pat_env
                            .names
                            .iter()
                            .find(|(_, v)| **v == id)
                            .map(|(k, _)| k)
                        {
                            new_env.insert(id, name.clone(), scheme);
                        }
                    }
                }
            }
        }

        Ok(new_env)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Module-level type checking
// ─────────────────────────────────────────────────────────────────────────────

/// Result of type checking a module.
#[derive(Debug)]
pub struct CheckResult {
    /// Type errors found during checking.
    pub errors: Vec<BoxedTypeError>,
    /// The typed module (with types filled in on expressions).
    pub module: crate::ast::Module,
}

impl CheckResult {
    /// Returns true if there were no errors.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Returns the errors, consuming the result.
    #[must_use]
    pub fn into_errors(self) -> Vec<BoxedTypeError> {
        self.errors
    }
}

/// Check a module for type errors.
///
/// This function performs module-level type inference:
/// 1. Collects all function signatures into the type environment
/// 2. Type-checks each function body
/// 3. Verifies return types match declared types
/// 4. Returns all accumulated type errors
///
/// # Example
///
/// ```ignore
/// let module = ambient_parser::parse(source)?;
/// let result = check_module(module);
/// if !result.is_ok() {
///     for error in &result.errors {
///         eprintln!("Type error: {}", error);
///     }
/// }
/// ```
#[must_use]
pub fn check_module(mut module: crate::ast::Module) -> CheckResult {
    let mut infer = Infer::new();
    let mut errors = Vec::new();
    let mut env = TypeEnv::new();

    // Phase 1: Collect all function signatures into the environment.
    // This allows functions to call each other regardless of definition order.
    let mut function_schemes: Vec<(BindingId, Arc<str>, Scheme)> = Vec::new();
    let mut next_binding_id: BindingId = 1_000_000; // Start high to avoid collisions

    for item in &module.items {
        if let crate::ast::ItemKind::Function(func) = &item.kind {
            let binding_id = next_binding_id;
            next_binding_id += 1;

            // Build the function type from its signature
            let scheme = build_function_scheme(&mut infer, func);
            function_schemes.push((binding_id, Arc::clone(&func.name), scheme));
        }
    }

    // Add all function schemes to the environment
    for (id, name, scheme) in &function_schemes {
        env.insert(*id, Arc::clone(name), scheme.clone());
    }

    // Phase 2: Type-check each function body.
    for item in &mut module.items {
        if let crate::ast::ItemKind::Function(func) = &mut item.kind {
            // Reset ability tracking for each function
            infer.reset_abilities();

            // Create function-local environment with parameters
            let mut func_env = env.extend();

            // Build expected return type
            let expected_ret_ty = func.ret_ty.clone().map(|ty| infer.resolve_holes(&ty));

            // Add parameters to the environment
            let mut param_types = Vec::new();
            for param in &func.params {
                let param_ty = match &param.ty {
                    Some(ty) => infer.resolve_holes(ty),
                    None => infer.fresh(),
                };
                param_types.push(param_ty.clone());
                func_env.insert_mono(param.id, Arc::clone(&param.name), param_ty);
            }

            // Infer the body type
            match infer.infer_expr(&func_env, &mut func.body) {
                Ok(body_ty) => {
                    // Check return type matches if declared
                    if let Some(ref expected) = expected_ret_ty {
                        let span = (func.body.span.start, func.body.span.end);
                        if let Err(e) = infer.unify(expected, &body_ty, span) {
                            errors.push(e.with_context(format!(
                                "in function `{}`: return type mismatch",
                                func.name
                            )));
                        }
                    }

                    // Verify declared abilities match inferred abilities
                    let inferred_abilities = infer.current_abilities().clone();
                    if !func.abilities.is_empty() {
                        // Convert declared abilities to AbilitySet
                        let declared: Vec<AbilityId> = func
                            .abilities
                            .iter()
                            .filter_map(|qn| infer.ability_name_to_id(&qn.name))
                            .collect();
                        let declared_set = AbilitySet::from_abilities(declared);

                        // Check that inferred abilities are a subset of declared
                        if let AbilitySet::Concrete(inferred_ids) = &inferred_abilities {
                            for ability_id in inferred_ids {
                                if !declared_set.contains(*ability_id) {
                                    let span = (item.span.start, item.span.end);
                                    errors.push(Box::new(
                                        TypeError::new(
                                            TypeErrorKind::MissingAbility {
                                                required: *ability_id,
                                                available: declared_set.clone(),
                                            },
                                            span,
                                        )
                                        .with_context(
                                            format!(
                                        "function `{}` uses ability #{} but doesn't declare it",
                                        func.name, ability_id
                                    ),
                                        ),
                                    ));
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    errors.push(e.with_context(format!("in function `{}`", func.name)));
                }
            }
        }

        // Type-check constants
        if let crate::ast::ItemKind::Const(const_def) = &mut item.kind {
            infer.reset_abilities();
            let expected_ty = infer.resolve_holes(&const_def.ty);

            match infer.infer_expr(&env, &mut const_def.value) {
                Ok(actual_ty) => {
                    let span = (const_def.value.span.start, const_def.value.span.end);
                    if let Err(e) = infer.unify(&expected_ty, &actual_ty, span) {
                        errors.push(e.with_context(format!(
                            "in constant `{}`: type mismatch",
                            const_def.name
                        )));
                    }
                }
                Err(e) => {
                    errors.push(e.with_context(format!("in constant `{}`", const_def.name)));
                }
            }
        }
    }

    CheckResult { errors, module }
}

/// Build a type scheme for a function from its signature.
fn build_function_scheme(infer: &mut Infer, func: &crate::ast::FunctionDef) -> Scheme {
    // Collect type variables from type parameters
    let mut type_var_map: std::collections::HashMap<Arc<str>, TypeVarId> =
        std::collections::HashMap::new();
    let mut quantified_vars = Vec::new();

    for (idx, tp) in func.type_params.iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let var_id = idx as TypeVarId;
        type_var_map.insert(Arc::clone(&tp.name), var_id);
        quantified_vars.push(var_id);
    }

    // Build parameter types
    let param_types: Vec<Type> = func
        .params
        .iter()
        .map(|p| match &p.ty {
            Some(ty) => substitute_type_params(ty, &type_var_map),
            None => infer.fresh(),
        })
        .collect();

    // Build return type
    let ret_ty = match &func.ret_ty {
        Some(ty) => substitute_type_params(ty, &type_var_map),
        None => infer.fresh(),
    };

    // Build ability set from declared abilities
    let abilities = if func.abilities.is_empty() {
        AbilitySet::Empty
    } else {
        let ability_ids: Vec<AbilityId> = func
            .abilities
            .iter()
            .filter_map(|qn| infer.ability_name_to_id(&qn.name))
            .collect();
        AbilitySet::from_abilities(ability_ids)
    };

    let fn_ty = Type::function_with_abilities(param_types, ret_ty, abilities);

    if quantified_vars.is_empty() {
        Scheme::mono(fn_ty)
    } else {
        Scheme::poly(quantified_vars, fn_ty)
    }
}

/// Substitute type parameters in a type with type variables.
fn substitute_type_params(
    ty: &Type,
    type_var_map: &std::collections::HashMap<Arc<str>, TypeVarId>,
) -> Type {
    match ty {
        Type::Named(named) => {
            // Check if this is a type parameter reference
            if named.args.is_empty() {
                if let Some(&var_id) = type_var_map.get(&named.name) {
                    return Type::var(var_id);
                }
            }
            // Otherwise, recursively substitute in type arguments
            Type::Named(crate::types::NamedType::new(
                Arc::clone(&named.name),
                named
                    .args
                    .iter()
                    .map(|arg| substitute_type_params(arg, type_var_map))
                    .collect(),
            ))
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
        // Primitives and other types pass through unchanged
        _ => ty.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOp, Param};

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

    #[test]
    fn test_infer_literal() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::number(42.0);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Number);

        let mut expr = Expr::string("hello");
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::String);

        let mut expr = Expr::bool(true);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Bool);
    }

    #[test]
    fn test_infer_binary_arithmetic() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::binary(BinaryOp::Add, Expr::number(1.0), Expr::number(2.0));
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Number);
    }

    #[test]
    fn test_infer_binary_comparison() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::binary(BinaryOp::Lt, Expr::number(1.0), Expr::number(2.0));
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Bool);
    }

    #[test]
    fn test_infer_if_then_else() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr =
            Expr::if_then_else(Expr::bool(true), Expr::number(1.0), Some(Expr::number(2.0)));
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Number);
    }

    #[test]
    fn test_infer_if_then_else_mismatch() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::if_then_else(
            Expr::bool(true),
            Expr::number(1.0),
            Some(Expr::string("hello")),
        );
        assert!(infer.infer_expr(&env, &mut expr).is_err());
    }

    #[test]
    fn test_infer_tuple() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::tuple(vec![Expr::number(1.0), Expr::string("hello")]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Tuple(vec![Type::Number, Type::String]));
    }

    #[test]
    fn test_infer_record() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        let mut expr = Expr::record([("x", Expr::number(1.0)), ("y", Expr::number(2.0))]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::record([("x", Type::Number), ("y", Type::Number)]));
    }

    #[test]
    fn test_infer_lambda() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // (x) => x + 1
        let mut expr = Expr::lambda(
            vec![Param::new(0, "x")],
            Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(1.0)),
        );
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        let ty = infer.apply(&ty);
        assert_eq!(ty, Type::function(vec![Type::Number], Type::Number));
    }

    #[test]
    fn test_infer_lambda_call() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // ((x) => x + 1)(42)
        let lambda = Expr::lambda(
            vec![Param::new(0, "x")],
            Expr::binary(BinaryOp::Add, Expr::local(0), Expr::number(1.0)),
        );
        let mut expr = Expr::call(lambda, vec![Expr::number(42.0)]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Number);
    }

    #[test]
    fn test_infer_let_polymorphism() {
        let mut infer = Infer::new();
        let mut env = TypeEnv::new();

        // identity: forall a. a -> a
        env.insert(
            0,
            "id".into(),
            Scheme::poly(vec![0], Type::function(vec![Type::var(0)], Type::var(0))),
        );

        // id(42) should be number
        let mut expr = Expr::call(Expr::local(0), vec![Expr::number(42.0)]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::Number);

        // id("hello") should be string
        let mut expr = Expr::call(Expr::local(0), vec![Expr::string("hello")]);
        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert_eq!(ty, Type::String);
    }

    #[test]
    fn test_generalize() {
        let infer = Infer::new();
        let env = TypeEnv::new();

        // A type with free variable should generalize
        let ty = Type::function(vec![Type::var(0)], Type::var(0));
        let scheme = infer.generalize(&env, &ty);

        assert_eq!(scheme.vars, vec![0]);
    }

    #[test]
    fn test_instantiate() {
        let mut infer = Infer::new();

        let scheme = Scheme::poly(vec![0], Type::function(vec![Type::var(0)], Type::var(0)));
        let ty = infer.instantiate(&scheme);

        // Should get a fresh type variable, not '0
        if let Type::Function(f) = ty {
            assert!(matches!(f.params[0], Type::Var(TypeVar::Unbound(_))));
            assert!(matches!(*f.ret, Type::Var(TypeVar::Unbound(_))));
        } else {
            panic!("Expected function type");
        }
    }

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

    // ─────────────────────────────────────────────────────────────────────────
    // Ability type inference tests (Milestone 8)
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
        let a = AbilitySet::from_abilities([1, 2]);
        let b = AbilitySet::from_abilities([1, 2]);
        let result = infer.unify_abilities(&a, &b, span());
        assert!(result.is_ok());
    }

    #[test]
    fn test_unify_different_abilities_fails() {
        let mut infer = Infer::new();
        let a = AbilitySet::from_abilities([1, 2]);
        let b = AbilitySet::from_abilities([1, 3]);
        let result = infer.unify_abilities(&a, &b, span());
        assert!(result.is_err());
    }

    #[test]
    fn test_unify_ability_var_with_concrete() {
        let mut infer = Infer::new();
        let var = AbilitySet::var(0);
        let concrete = AbilitySet::from_abilities([1, 2]);
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
    fn test_ability_tracking() {
        let mut infer = Infer::new();

        // Start with empty abilities
        assert!(infer.current_abilities().is_pure());

        // Require an ability
        infer.require_ability(1);
        assert!(infer.current_abilities().contains(1));

        // Require another ability
        infer.require_ability(2);
        assert!(infer.current_abilities().contains(1));
        assert!(infer.current_abilities().contains(2));

        // Reset
        infer.reset_abilities();
        assert!(infer.current_abilities().is_pure());
    }

    #[test]
    fn test_fresh_ability_var() {
        let mut infer = Infer::new();
        let v1 = infer.fresh_ability_var();
        let v2 = infer.fresh_ability_var();

        assert!(matches!(v1, AbilitySet::Var(0)));
        assert!(matches!(v2, AbilitySet::Var(1)));
    }

    #[test]
    fn test_apply_abilities() {
        let mut infer = Infer::new();
        let var = AbilitySet::var(0);
        let concrete = AbilitySet::from_abilities([1, 2]);

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
            assert!(matches!(f.params[0], Type::Var(TypeVar::Unbound(id)) if id != 100));
            assert!(matches!(f.abilities, AbilitySet::Var(id) if id != 100));
        } else {
            panic!("Expected function type");
        }
    }

    #[test]
    fn test_unify_functions_with_abilities() {
        let mut infer = Infer::new();

        let f1 = Type::function_with_abilities(
            vec![Type::Number],
            Type::String,
            AbilitySet::from_abilities([1]),
        );

        let f2 = Type::function_with_abilities(
            vec![Type::Number],
            Type::String,
            AbilitySet::from_abilities([1]),
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
            AbilitySet::from_abilities([1]),
        );

        let f2 = Type::function_with_abilities(
            vec![Type::Number],
            Type::String,
            AbilitySet::from_abilities([2]),
        );

        let result = infer.unify(&f1, &f2, span());
        assert!(result.is_err());
    }

    #[test]
    fn test_unify_ability_values() {
        let mut infer = Infer::new();

        let av1 = Type::ability_value(Type::String, AbilitySet::single(1));
        let av2 = Type::ability_value(Type::String, AbilitySet::single(1));

        let result = infer.unify(&av1, &av2, span());
        assert!(result.is_ok());
    }

    #[test]
    fn test_unify_ability_values_different_result_fails() {
        let mut infer = Infer::new();

        let av1 = Type::ability_value(Type::String, AbilitySet::single(1));
        let av2 = Type::ability_value(Type::Number, AbilitySet::single(1));

        let result = infer.unify(&av1, &av2, span());
        assert!(result.is_err());
    }

    #[test]
    fn test_ability_name_to_id() {
        let infer = Infer::new();
        assert_eq!(infer.ability_name_to_id("Console"), Some(1));
        assert_eq!(infer.ability_name_to_id("Exception"), Some(2));
        assert_eq!(infer.ability_name_to_id("Time"), Some(3));
        assert_eq!(infer.ability_name_to_id("Random"), Some(4));
        assert_eq!(infer.ability_name_to_id("Async"), Some(5));
        assert_eq!(infer.ability_name_to_id("Unknown"), None);
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
    fn test_resolve_holes_simple() {
        let mut infer = Infer::new();

        // Hole becomes a fresh type variable
        let resolved = infer.resolve_holes(&Type::Hole);
        assert!(matches!(resolved, Type::Var(TypeVar::Unbound(_))));
    }

    #[test]
    fn test_resolve_holes_nested() {
        let mut infer = Infer::new();

        // Holes in nested types get resolved
        let func = Type::function(vec![Type::Hole], Type::Hole);
        let resolved = infer.resolve_holes(&func);

        if let Type::Function(f) = resolved {
            assert!(matches!(f.params[0], Type::Var(TypeVar::Unbound(_))));
            assert!(matches!(*f.ret, Type::Var(TypeVar::Unbound(_))));
        } else {
            panic!("Expected function type");
        }
    }

    #[test]
    fn test_resolve_holes_partial() {
        let mut infer = Infer::new();

        // Mix of concrete types and holes
        let tuple = Type::Tuple(vec![Type::Number, Type::Hole, Type::String]);
        let resolved = infer.resolve_holes(&tuple);

        if let Type::Tuple(elems) = resolved {
            assert_eq!(elems[0], Type::Number);
            assert!(matches!(elems[1], Type::Var(TypeVar::Unbound(_))));
            assert_eq!(elems[2], Type::String);
        } else {
            panic!("Expected tuple type");
        }
    }

    #[test]
    fn test_require_ability_with_registry() {
        use crate::types::{AbilityInfo, AbilityRegistry};

        let mut registry = AbilityRegistry::new();

        // IO is ability 1
        registry.register(1, AbilityInfo::new("IO"));

        // FileSystem (2) depends on IO (1)
        registry.register(2, AbilityInfo::new("FileSystem").with_dependency(1));

        let mut infer = Infer::with_registry(registry);

        // When we require FileSystem, IO should also be required
        infer.require_ability(2);

        let abilities = infer.current_abilities();
        if let AbilitySet::Concrete(ids) = abilities {
            assert!(ids.contains(&1), "IO should be required");
            assert!(ids.contains(&2), "FileSystem should be required");
        } else {
            panic!("Expected concrete ability set");
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Async type checking tests (Milestone 9)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_async_all_type_inference() {
        let mut infer = Infer::new();

        // Create argument type: List<Ability<string, Console!>>
        let ability_value = Type::ability_value(Type::String, AbilitySet::single(1)); // Console = 1
        let list_of_abilities = Type::named("List", vec![ability_value]);

        // Look up Async.all with this argument
        let result = infer.lookup_ability_method("Async", "all", &[list_of_abilities], span());
        assert!(
            result.is_ok(),
            "Async.all should accept List<Ability<T, A!>>"
        );

        let (ability_id, result_ty, additional_abilities) = result.unwrap();

        // Should return Async ability ID
        assert_eq!(ability_id, 5, "Should return Async ability ID");

        // Should return List<string> (the result type wrapped in List)
        if let Type::Named(named) = &result_ty {
            assert_eq!(named.name.as_ref(), "List");
            assert_eq!(named.args.len(), 1);
            assert_eq!(named.args[0], Type::String);
        } else {
            panic!("Expected Named type List<string>, got {:?}", result_ty);
        }

        // Should include Console in additional abilities
        assert!(
            matches!(&additional_abilities, AbilitySet::Concrete(ids) if ids.contains(&1)),
            "Should include Console ability in additional_abilities"
        );
    }

    #[test]
    fn test_async_race_type_inference() {
        let mut infer = Infer::new();

        // Create argument type: List<Ability<number, Time!>>
        let ability_value = Type::ability_value(Type::Number, AbilitySet::single(3)); // Time = 3
        let list_of_abilities = Type::named("List", vec![ability_value]);

        // Look up Async.race with this argument
        let result = infer.lookup_ability_method("Async", "race", &[list_of_abilities], span());
        assert!(
            result.is_ok(),
            "Async.race should accept List<Ability<T, A!>>"
        );

        let (ability_id, result_ty, additional_abilities) = result.unwrap();

        // Should return Async ability ID
        assert_eq!(ability_id, 5, "Should return Async ability ID");

        // Should return number (the unwrapped result type)
        assert_eq!(
            result_ty,
            Type::Number,
            "Async.race should return T, not List<T>"
        );

        // Should include Time in additional abilities
        assert!(
            matches!(&additional_abilities, AbilitySet::Concrete(ids) if ids.contains(&3)),
            "Should include Time ability in additional_abilities"
        );
    }

    #[test]
    fn test_async_all_with_type_variable() {
        let mut infer = Infer::new();

        // Create a type variable for the result type
        let result_var = infer.fresh();
        let ability_var = infer.fresh_ability_var();

        // Create argument type: List<Ability<T, A!>> with fresh variables
        let ability_value = Type::ability_value(result_var.clone(), ability_var.clone());
        let list_of_abilities = Type::named("List", vec![ability_value]);

        // Look up Async.all - should succeed with polymorphic types
        let result = infer.lookup_ability_method("Async", "all", &[list_of_abilities], span());
        assert!(result.is_ok(), "Async.all should work with type variables");

        let (_, result_ty, _) = result.unwrap();

        // Result should be List<T> where T is the same variable
        if let Type::Named(named) = &result_ty {
            assert_eq!(named.name.as_ref(), "List");
            // The inner type should be related to our original type variable
            // (either the same or unified)
        } else {
            panic!("Expected Named type, got {:?}", result_ty);
        }
    }

    #[test]
    fn test_async_all_wrong_arity() {
        let mut infer = Infer::new();

        // Try calling Async.all with no arguments
        let result = infer.lookup_ability_method("Async", "all", &[], span());
        assert!(
            result.is_err(),
            "Async.all should require exactly one argument"
        );

        // Try calling Async.all with two arguments
        let arg1 = Type::named(
            "List",
            vec![Type::ability_value(Type::String, AbilitySet::single(1))],
        );
        let arg2 = Type::named(
            "List",
            vec![Type::ability_value(Type::Number, AbilitySet::single(1))],
        );
        let result = infer.lookup_ability_method("Async", "all", &[arg1, arg2], span());
        assert!(result.is_err(), "Async.all should not accept two arguments");
    }

    #[test]
    fn test_async_race_wrong_arity() {
        let mut infer = Infer::new();

        // Try calling Async.race with no arguments
        let result = infer.lookup_ability_method("Async", "race", &[], span());
        assert!(
            result.is_err(),
            "Async.race should require exactly one argument"
        );
    }

    #[test]
    fn test_async_all_wrong_type() {
        let mut infer = Infer::new();

        // Try calling Async.all with a non-List type (e.g., just a number)
        let result = infer.lookup_ability_method("Async", "all", &[Type::Number], span());
        assert!(
            result.is_err(),
            "Async.all should reject non-List arguments"
        );

        // Try calling Async.all with List<number> (not List<Ability<...>>)
        let list_of_numbers = Type::named("List", vec![Type::Number]);
        let result = infer.lookup_ability_method("Async", "all", &[list_of_numbers], span());
        assert!(result.is_err(), "Async.all should reject List<number>");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Handler literal type checking tests (Milestone 13)
    // ─────────────────────────────────────────────────────────────────────────

    use crate::ast::HandlerLiteralMethod;

    #[test]
    fn test_handler_literal_console_print() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // { print(msg) => resume(()) }
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "print",
            vec![Param::new(1, "msg")],
            Expr::unit(), // resume(()) - simplified for test
        )]);

        let ty = infer.infer_expr(&env, &mut expr).unwrap();

        // Should infer Handler<Console>
        if let Type::Handler(handler_ty) = ty {
            assert_eq!(handler_ty.ability, 0x0001); // Console ability ID
        } else {
            panic!("Expected Handler type, got {:?}", ty);
        }
    }

    #[test]
    fn test_handler_literal_exception_throw() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // { throw(err) => ... }
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "throw",
            vec![Param::new(1, "err")],
            Expr::unit(),
        )]);

        let ty = infer.infer_expr(&env, &mut expr).unwrap();

        // Should infer Handler<Exception>
        if let Type::Handler(handler_ty) = ty {
            assert_eq!(handler_ty.ability, 0x0002); // Exception ability ID
        } else {
            panic!("Expected Handler type, got {:?}", ty);
        }
    }

    #[test]
    fn test_handler_literal_time_methods() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // { now() => resume(0.0), wait(duration) => resume(()) }
        let mut expr = Expr::handler_literal(vec![
            HandlerLiteralMethod::new("now", vec![], Expr::number(0.0)),
            HandlerLiteralMethod::new("wait", vec![Param::new(1, "duration")], Expr::unit()),
        ]);

        let ty = infer.infer_expr(&env, &mut expr).unwrap();

        // Should infer Handler<Time>
        if let Type::Handler(handler_ty) = ty {
            assert_eq!(handler_ty.ability, 0x0003); // Time ability ID
        } else {
            panic!("Expected Handler type, got {:?}", ty);
        }
    }

    #[test]
    fn test_handler_literal_unknown_method() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // { unknown_method(x) => ... } - doesn't match any ability
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "unknown_method",
            vec![Param::new(1, "x")],
            Expr::unit(),
        )]);

        let result = infer.infer_expr(&env, &mut expr);
        assert!(
            result.is_err(),
            "Should fail when methods don't match any ability"
        );
    }

    #[test]
    fn test_handler_literal_wrong_arity() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // { print(a, b) => ... } - Console.print takes 1 arg, not 2
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "print",
            vec![Param::new(1, "a"), Param::new(2, "b")],
            Expr::unit(),
        )]);

        let result = infer.infer_expr(&env, &mut expr);
        assert!(result.is_err(), "Should fail when arity doesn't match");

        // Check error message mentions arity
        if let Err(err) = result {
            let msg = format!("{}", err.kind);
            assert!(
                msg.contains("expects 1 parameters") || msg.contains("expected 1"),
                "Error should mention expected arity: {}",
                msg
            );
        }
    }

    #[test]
    fn test_handler_literal_partial_handler() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // { print(msg) => ... } - only handles print, not println/eprint
        // This should be allowed (partial handlers can be composed)
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "print",
            vec![Param::new(1, "msg")],
            Expr::unit(),
        )]);

        let ty = infer.infer_expr(&env, &mut expr).unwrap();
        assert!(
            matches!(ty, Type::Handler(_)),
            "Partial handlers should be allowed"
        );
    }

    #[test]
    fn test_handler_literal_method_body_type_checked() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // { print(msg) => msg + 1 } - body uses msg (should type-check)
        let mut expr = Expr::handler_literal(vec![HandlerLiteralMethod::new(
            "print",
            vec![Param::new(1, "msg")],
            Expr::binary(BinaryOp::Add, Expr::local(1), Expr::number(1.0)),
        )]);

        // This should succeed - the parameter 'msg' is in scope
        let result = infer.infer_expr(&env, &mut expr);
        assert!(
            result.is_ok(),
            "Handler method body should type-check with params in scope"
        );
    }

    #[test]
    fn test_infer_ability_from_methods_uniqueness() {
        let infer = Infer::new();

        // "print" exists in Console
        let methods: Vec<Arc<str>> = vec!["print".into()];
        let ability = infer.infer_ability_from_methods(&methods);
        assert_eq!(ability, Some(0x0001)); // Console

        // "throw" exists only in Exception
        let methods: Vec<Arc<str>> = vec!["throw".into()];
        let ability = infer.infer_ability_from_methods(&methods);
        assert_eq!(ability, Some(0x0002)); // Exception

        // "now" exists only in Time
        let methods: Vec<Arc<str>> = vec!["now".into()];
        let ability = infer.infer_ability_from_methods(&methods);
        assert_eq!(ability, Some(0x0003)); // Time
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Error case coverage tests (CQ-012)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_error_undefined_variable() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // Reference to a variable that doesn't exist
        let mut expr = Expr::variable("undefined_var");
        let result = infer.infer_expr(&env, &mut expr);

        assert!(result.is_err());
        if let Err(err) = result {
            assert!(
                matches!(err.kind, TypeErrorKind::UndefinedVariable { .. }),
                "Expected UndefinedVariable, got {:?}",
                err.kind
            );
        }
    }

    #[test]
    fn test_error_field_not_found() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // Access a field that doesn't exist on a record
        let record = Expr::record([("x", Expr::number(1.0)), ("y", Expr::number(2.0))]);
        let mut expr = Expr::field_access(record, "z");
        let result = infer.infer_expr(&env, &mut expr);

        assert!(result.is_err());
        if let Err(err) = result {
            assert!(
                matches!(err.kind, TypeErrorKind::FieldNotFound { .. }),
                "Expected FieldNotFound, got {:?}",
                err.kind
            );
        }
    }

    #[test]
    fn test_error_tuple_index_out_of_bounds() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // Access index 5 on a 2-element tuple
        let tuple = Expr::tuple(vec![Expr::number(1.0), Expr::number(2.0)]);
        let mut expr = Expr::tuple_index(tuple, 5);
        let result = infer.infer_expr(&env, &mut expr);

        assert!(result.is_err());
        if let Err(err) = result {
            assert!(
                matches!(err.kind, TypeErrorKind::TupleIndexOutOfBounds { .. }),
                "Expected TupleIndexOutOfBounds, got {:?}",
                err.kind
            );
        }
    }

    #[test]
    fn test_error_calling_non_function() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // Try to call a number as a function - type inference will produce TypeMismatch
        // because it tries to unify Number with Function type
        let mut expr = Expr::call(Expr::number(42.0), vec![Expr::number(1.0)]);
        let result = infer.infer_expr(&env, &mut expr);

        assert!(result.is_err());
        if let Err(err) = result {
            // The error is TypeMismatch because unification fails when trying
            // to match Number with a function type
            assert!(
                matches!(err.kind, TypeErrorKind::TypeMismatch { .. }),
                "Expected TypeMismatch when calling non-function, got {:?}",
                err.kind
            );
        }
    }

    #[test]
    fn test_error_non_boolean_if_condition() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // if with a number condition instead of bool - produces TypeMismatch
        // when unifying condition type (Number) with Bool
        let mut expr = Expr::if_then_else(
            Expr::number(1.0),
            Expr::number(2.0),
            Some(Expr::number(3.0)),
        );
        let result = infer.infer_expr(&env, &mut expr);

        assert!(result.is_err());
        if let Err(err) = result {
            // Unification error: expected Number (condition type), actual Bool (target type)
            assert!(
                matches!(err.kind, TypeErrorKind::TypeMismatch { .. }),
                "Expected TypeMismatch for non-bool condition, got {:?}",
                err.kind
            );
        }
    }

    #[test]
    fn test_error_match_arms_different_types() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // Match with arms returning different types - produces TypeMismatch
        // when unifying first arm type with subsequent arm types
        use crate::ast::{MatchArm, Pattern};
        let mut expr = Expr::match_expr(
            Expr::number(1.0),
            vec![
                MatchArm::new(Pattern::wildcard(), Expr::number(1.0)),
                MatchArm::new(Pattern::wildcard(), Expr::string("hello")),
            ],
        );
        let result = infer.infer_expr(&env, &mut expr);

        assert!(result.is_err());
        if let Err(err) = result {
            assert!(
                matches!(
                    err.kind,
                    TypeErrorKind::TypeMismatch {
                        expected: Type::Number,
                        actual: Type::String
                    }
                ),
                "Expected TypeMismatch between Number and String, got {:?}",
                err.kind
            );
        }
    }

    #[test]
    fn test_error_wrong_argument_count() {
        let mut infer = Infer::new();
        let env = TypeEnv::new();

        // Call a function with wrong number of arguments - produces TypeMismatch
        // because function types don't match
        let lambda = Expr::lambda(
            vec![Param::new(0, "x"), Param::new(1, "y")],
            Expr::binary(BinaryOp::Add, Expr::local(0), Expr::local(1)),
        );
        let mut expr = Expr::call(lambda, vec![Expr::number(1.0)]); // Only 1 arg, needs 2
        let result = infer.infer_expr(&env, &mut expr);

        assert!(result.is_err());
        // This produces a TypeMismatch because the inferred function type
        // doesn't match the application
        if let Err(err) = result {
            assert!(
                matches!(err.kind, TypeErrorKind::TypeMismatch { .. }),
                "Expected TypeMismatch for wrong argument count, got {:?}",
                err.kind
            );
        }
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

//! Type error types for the Ambient type checker.

use std::sync::Arc;

use crate::ast::BindingId;
use crate::types::{AbilityId, AbilitySet, AbilityVarId, Type, TypeVarId};

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
pub(crate) fn type_error(kind: TypeErrorKind, span: (u32, u32)) -> BoxedTypeError {
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

    /// A `use` declaration failed to resolve.
    ImportFailed { message: String },

    /// A declaration is malformed in a way types alone don't capture
    /// (e.g. reusing a reserved prelude uuid with the wrong layout).
    InvalidDeclaration { message: String },

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

    /// A bare-method perform (`seed!(…)`) whose method name no ability
    /// method import brings into scope. `suggestion` carries a `use` path
    /// that would import it, when some in-scope ability declares a method
    /// of that name.
    UnimportedAbilityMethod {
        method: Arc<str>,
        suggestion: Option<Arc<str>>,
    },

    /// A declared ability (row) variable (`E!`) was used where a type is
    /// expected (e.g. `x: E`). An ability variable names an effect row, not
    /// a type.
    AbilityVarAsType { name: Arc<str> },

    /// A single `with` row named more than one ability (row) variable
    /// (`with E, F`). An effect row has one polymorphic tail, so at most one
    /// ability variable may appear in a row.
    MultipleRowVariables { first: Arc<str>, second: Arc<str> },

    /// Two or more abilities depend on each other through their `with`
    /// clauses, directly or transitively. The dependency graph must be
    /// acyclic: a method's identity folds in the default-implementation
    /// hashes of its declared dependencies, so a cycle would make a
    /// method's key depend on itself. `cycle` names the loop, the same
    /// ability at both ends (`A → B → A`).
    AbilityDependencyCycle { cycle: Vec<Arc<str>> },

    /// Ability not handled.
    AbilityNotHandled { ability: AbilityId },

    // ─────────────────────────────────────────────────────────────────────────
    // Handler literal errors (Milestone 13)
    // ─────────────────────────────────────────────────────────────────────────
    /// A handler *value* (`Handler<A, R>`) tried to cover more than one
    /// ability. Multi-ability braces are legal only directly in a `with`
    /// list; used as a value they must be split, one per ability.
    HandlerValueMultipleAbilities { abilities: Vec<Arc<str>> },

    /// A handler literal has no arms, so its ability can't be determined.
    HandlerEmpty,

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

    /// `resume` used outside a handler arm.
    ResumeOutsideHandler,

    /// `resume` used in a handler arm for a method that returns `!` —
    /// there is no continuation to feed, the perform site unwinds.
    ResumeNeverMethod { ability: Arc<str>, method: Arc<str> },

    // ─────────────────────────────────────────────────────────────────────────
    // Sandbox errors (Milestone 14)
    // ─────────────────────────────────────────────────────────────────────────
    /// Sandbox attempted to use an ability that is not allowed.
    SandboxAbilityViolation {
        ability: Arc<str>,
        allowed: Vec<Arc<str>>,
    },

    /// Ability requires a namespace prefix.
    AbilityRequiresNamespace {
        ability: Arc<str>,
        expected_namespace: Arc<str>,
    },

    // ─────────────────────────────────────────────────────────────────────────
    // Typed record errors
    // ─────────────────────────────────────────────────────────────────────────
    /// Unknown type name in typed record construction.
    UndefinedTypeName { name: Arc<str> },

    /// Type is not constructable as a record.
    NotARecordType { ty: Type },

    /// An `extern` (engine-provided) struct was constructed in Ambient code.
    /// Such a type may be named and read from, but only the engine may build
    /// its values.
    CannotConstructExtern { name: Arc<str> },

    // ─────────────────────────────────────────────────────────────────────────
    // Trait errors
    // ─────────────────────────────────────────────────────────────────────────
    /// Trait not found.
    UnknownTrait { name: Arc<str> },

    /// Trait is not implemented for a type.
    TraitNotImplemented { trait_name: Arc<str>, ty: Type },

    /// Method not found on type.
    MethodNotFound { method: Arc<str>, ty: Type },

    /// A pattern or expression referenced an unknown enum variant.
    UnknownVariant { name: Arc<str> },

    /// A variant pattern's payload shape doesn't match the declaration.
    VariantPayloadMismatch {
        variant: Arc<str>,
        expects_payload: bool,
    },

    /// Ambiguous method call (multiple traits provide the same method).
    AmbiguousMethod {
        method: Arc<str>,
        ty: Type,
        candidates: Vec<Arc<str>>,
    },

    /// Cannot implement trait for non-nominal type.
    TraitOnStructuralType { trait_name: Arc<str>, ty: Type },

    /// A bound `τ: Trait` failed: the type has no impl of the trait in the
    /// build.
    BoundNotSatisfied { ty: Type, trait_name: Arc<str> },

    /// A direct-dispatch site (operator, dot-call, or associated call) matched
    /// a generic impl by *head* identity — coherence granularity — but the
    /// impl's applied target does not cover this instantiation: `impl Eq for
    /// Option<Number>` matched against an `Option<String>` receiver. The impl
    /// is *found*, yet it implements only its own exact instantiation, so
    /// dispatching to it would be a silent misdispatch.
    ImplInstantiationNotCovered { ty: Type, trait_name: Arc<str> },

    /// A `State` cell write whose cell type mentions the enclosing item's
    /// type parameter: the migration fingerprint must mean exactly one
    /// type per perform site (see `ref/live-upgrade.md`, "Migration").
    GenericStateWrite { ty: Type },

    /// A generic body used a type parameter where a bound is required,
    /// but the parameter doesn't declare that bound.
    MissingParamBound {
        param: Arc<str>,
        trait_name: Arc<str>,
    },

    /// Solving a trait bound recursed through conditional impls past the
    /// depth limit (e.g. `impl<T: Eq> Eq for Pair<Pair<T>>` applied to an
    /// ever-nesting type), so it was cut off rather than looping forever.
    DictSolveDepthLimit { ty: Type, trait_name: Arc<str> },

    /// A bounded generic item was referenced as a first-class value
    /// (`let f = contains;`). Dictionaries are supplied at call sites, so
    /// bounded generics must (for now) be called directly.
    BoundedGenericAsValue { name: Arc<str> },

    /// A method call on a rigid type parameter found no method of that
    /// name in the parameter's bounds.
    MethodNotInBounds {
        method: Arc<str>,
        param: Arc<str>,
        bounds: Vec<Arc<str>>,
    },

    /// A second impl of the same trait for the same type.
    DuplicateImpl { trait_name: Arc<str>, ty: Type },

    /// Impl method signature doesn't match trait.
    ImplMethodSignatureMismatch {
        trait_name: Arc<str>,
        method: Arc<str>,
        expected: Type,
        actual: Type,
    },

    /// Impl is missing a required method.
    ImplMissingMethod {
        trait_name: Arc<str>,
        method: Arc<str>,
    },

    /// An inherent impl targets a type that cannot carry methods (a
    /// structural type, a bare type parameter, or an unknown name).
    InherentImplInvalidTarget { ty: Type },

    /// Two inherent definitions of the same method for the same type.
    /// The definitions would compete for one dispatch symbol.
    DuplicateInherentMethod { method: Arc<str>, ty: Type },

    /// An inherent method without a declared return type. Inherent method
    /// signatures are the dispatch contract (like `pub fn` signatures), so
    /// the return type must be written out.
    InherentMethodMissingReturnType { method: Arc<str> },

    /// A trait impl method declared a `with` clause; trait method effects
    /// are not part of trait signatures yet.
    AbilityClauseOnTraitImpl { method: Arc<str> },

    /// An impl method's method-level type-parameter bounds don't match the
    /// trait method's declared bounds. The bounds are the method's hidden
    /// dictionary calling convention, so they must line up exactly.
    TraitMethodBoundMismatch {
        /// The method name.
        method: Arc<str>,
        /// The trait's declared bounds, rendered `U: Eq` and joined.
        expected: String,
        /// The impl method's declared bounds, rendered the same way.
        found: String,
    },

    /// A `const` was initialized with something other than a literal value.
    /// Constants map an identifier to a single hashed primitive, so their
    /// initializer must be a literal (see `crate::const_eval`).
    ConstNotLiteral { name: Arc<str> },

    /// An unannotated parameter/return of a *generic* function was inferred
    /// to a type mentioning one of the function's own type parameters.
    /// Unannotated positions are monomorphic (one variable shared by the
    /// body and every call site), so they cannot carry a quantified
    /// parameter — the position must be annotated.
    GenericPositionNeedsAnnotation {
        func: Arc<str>,
        position: Arc<str>,
        inferred: Type,
    },

    /// A public function signature with an unannotated parameter or return
    /// type. A `pub fn` signature is the cross-module contract — importing
    /// modules rebuild the scheme from the written annotations alone, so an
    /// omitted type would let foreign callers bypass the body's types
    /// entirely.
    PublicFnMissingAnnotation { func: Arc<str>, position: Arc<str> },
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
            Self::ImportFailed { message } => {
                write!(f, "import failed: {message}")
            }
            Self::InvalidDeclaration { message } => {
                write!(f, "invalid declaration: {message}")
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
            Self::UnimportedAbilityMethod { method, suggestion } => {
                write!(f, "no ability method `{method}` in scope")?;
                if let Some(path) = suggestion {
                    write!(f, "; import it with `use {path};`")?;
                }
                Ok(())
            }
            Self::AbilityVarAsType { name } => {
                write!(
                    f,
                    "`{name}` is an ability variable, not a type; \
                     it may only appear in a `with` clause"
                )
            }
            Self::MultipleRowVariables { first, second } => {
                write!(
                    f,
                    "a `with` row may name at most one ability variable, \
                     but this row names both `{first}` and `{second}`"
                )
            }
            Self::AbilityDependencyCycle { cycle } => {
                let path = cycle
                    .iter()
                    .map(AsRef::as_ref)
                    .collect::<Vec<_>>()
                    .join(" → ");
                write!(
                    f,
                    "ability dependency cycle: {path}. Abilities may not depend on \
                     each other through `with`, directly or transitively"
                )
            }
            Self::AbilityNotHandled { ability } => {
                write!(f, "ability #{ability} is not handled")
            }
            Self::HandlerValueMultipleAbilities { abilities } => {
                let abilities = abilities
                    .iter()
                    .map(AsRef::as_ref)
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "a handler value handles one ability, but this literal covers [{abilities}]. \
                     Use it directly in a `with` list, or split it into one handler per ability"
                )
            }
            Self::HandlerEmpty => {
                write!(
                    f,
                    "an empty handler literal `{{}}` has no ability; add at least one \
                     `Ability::method(...) => ...` arm"
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
                    "handler method `{method}` for ability `{ability}` expects \
                     {expected} parameters, but handler provides {actual}"
                )
            }
            Self::HandlerMissingMethod { ability, method } => {
                write!(
                    f,
                    "handler for `{ability}` is missing required method `{method}`"
                )
            }
            Self::ResumeOutsideHandler => {
                write!(
                    f,
                    "`resume` is only meaningful inside a handler arm, where it \
                     continues the suspended computation"
                )
            }
            Self::ResumeNeverMethod { ability, method } => {
                write!(
                    f,
                    "cannot `resume` `{ability}::{method}`: it returns `!` (never), \
                     so the perform site unwinds and there is nothing to resume — \
                     yield a value from the arm instead"
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
            Self::AbilityRequiresNamespace {
                ability,
                expected_namespace,
            } => {
                write!(
                    f,
                    "ability `{ability}` is not in scope bare: qualify it as \
                     `{expected_namespace}::{ability}`, or import it with \
                     `use {expected_namespace}::{ability};`"
                )
            }
            Self::UndefinedTypeName { name } => {
                write!(f, "undefined type: `{name}`")
            }
            Self::NotARecordType { ty } => {
                write!(
                    f,
                    "type `{ty}` is not a record type and cannot be constructed with {{ field: value }} syntax"
                )
            }
            Self::CannotConstructExtern { name } => {
                write!(
                    f,
                    "type `{name}` is provided by the engine and cannot be constructed by Ambient code"
                )
            }
            Self::UnknownTrait { name } => {
                write!(f, "unknown trait: `{name}`")
            }
            Self::TraitNotImplemented { trait_name, ty } => {
                write!(f, "trait `{trait_name}` is not implemented for `{ty}`")
            }
            Self::UnknownVariant { name } => {
                write!(f, "unknown enum variant: `{name}`")
            }
            Self::VariantPayloadMismatch {
                variant,
                expects_payload,
            } => {
                if *expects_payload {
                    write!(
                        f,
                        "variant `{variant}` carries a payload; use `{variant}(pattern)`"
                    )
                } else {
                    write!(f, "variant `{variant}` has no payload")
                }
            }
            Self::MethodNotFound { method, ty } => {
                write!(f, "method `{method}` not found for type `{ty}`")
            }
            Self::AmbiguousMethod {
                method,
                ty,
                candidates,
            } => {
                let traits = candidates
                    .iter()
                    .map(|s| format!("`{s}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "ambiguous method `{method}` for type `{ty}`: could be from {traits}"
                )
            }
            Self::DuplicateImpl { trait_name, ty } => {
                write!(
                    f,
                    "duplicate implementation of trait `{trait_name}` for `{ty}` \
                     (each trait may be implemented at most once per type in a package)"
                )
            }
            Self::TraitOnStructuralType { trait_name, ty } => {
                write!(
                    f,
                    "cannot implement trait `{trait_name}` for structural type `{ty}`; traits can only be implemented for nominal types"
                )
            }
            Self::BoundNotSatisfied { ty, trait_name } => {
                write!(
                    f,
                    "the trait bound `{ty}: {trait_name}` is not satisfied; \
                     no `impl {trait_name} for {ty}` exists in this build"
                )
            }
            Self::ImplInstantiationNotCovered { ty, trait_name } => {
                write!(
                    f,
                    "the `{trait_name}` impl in scope does not cover `{ty}`: an \
                     applied impl (e.g. `impl {trait_name} for Option<Number>`) \
                     implements only its exact instantiation, not `{ty}`"
                )
            }
            Self::GenericStateWrite { ty } => {
                write!(
                    f,
                    "cannot fingerprint a `State` cell at generic type `{ty}`: \
                     the cell type must be concrete at each write (call this \
                     through a concretely-typed wrapper, or annotate the value)"
                )
            }
            Self::MissingParamBound { param, trait_name } => {
                write!(
                    f,
                    "type parameter `{param}` does not declare the bound `{trait_name}` \
                     required here; add it (`{param}: {trait_name}`)"
                )
            }
            Self::DictSolveDepthLimit { ty, trait_name } => {
                write!(
                    f,
                    "resolving the trait bound `{ty}: {trait_name}` recursed too deeply \
                     through conditional impls; the nesting has no base case (an \
                     ever-growing type), so it was cut off"
                )
            }
            Self::BoundedGenericAsValue { name } => {
                write!(
                    f,
                    "`{name}` has trait bounds, so it cannot be used as a first-class \
                     value yet; call it directly instead"
                )
            }
            Self::MethodNotInBounds {
                method,
                param,
                bounds,
            } => {
                if bounds.is_empty() {
                    write!(
                        f,
                        "no method `{method}` on type parameter `{param}`; \
                         `{param}` declares no trait bounds, so no methods are callable on it"
                    )
                } else {
                    let list: Vec<&str> = bounds.iter().map(AsRef::as_ref).collect();
                    write!(
                        f,
                        "no method `{method}` on type parameter `{param}`; \
                         its bounds ({}) do not provide it",
                        list.join(", ")
                    )
                }
            }
            Self::ImplMethodSignatureMismatch {
                trait_name,
                method,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "method `{method}` in impl for `{trait_name}` has wrong type: expected `{expected}`, found `{actual}`"
                )
            }
            Self::ImplMissingMethod { trait_name, method } => {
                write!(
                    f,
                    "impl for `{trait_name}` is missing required method `{method}`"
                )
            }
            Self::InherentImplInvalidTarget { ty } => {
                write!(
                    f,
                    "inherent `impl` target must be a nominal type, an enum, or a built-in type; `{ty}` is none of these"
                )
            }
            Self::DuplicateInherentMethod { method, ty } => {
                write!(
                    f,
                    "duplicate inherent method `{method}` for `{ty}`: only one definition per type and method name is allowed"
                )
            }
            Self::InherentMethodMissingReturnType { method } => {
                write!(f, "inherent method `{method}` must declare a return type")
            }
            Self::AbilityClauseOnTraitImpl { method } => {
                write!(
                    f,
                    "trait impl method `{method}` cannot declare a `with` clause: trait method effects are fixed by the trait signature"
                )
            }
            Self::TraitMethodBoundMismatch {
                method,
                expected,
                found,
            } => {
                write!(
                    f,
                    "impl method `{method}` must declare the same method-level trait bounds \
                     as the trait: expected [{expected}], found [{found}]"
                )
            }
            Self::ConstNotLiteral { name } => {
                write!(
                    f,
                    "`const {name}` must be initialized with a literal value (a number, string, boolean, or `()`)"
                )
            }
            Self::GenericPositionNeedsAnnotation {
                func,
                position,
                inferred,
            } => {
                write!(
                    f,
                    "in generic function `{func}`: the {position} is inferred as `{inferred}`, \
                     which mentions a type parameter; an unannotated position is monomorphic, \
                     so it must be annotated in the signature"
                )
            }
            Self::PublicFnMissingAnnotation { func, position } => {
                write!(
                    f,
                    "public function `{func}` must declare {position}: a `pub` signature \
                     is the contract other modules compile against"
                )
            }
        }
    }
}

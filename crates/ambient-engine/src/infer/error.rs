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
            Self::AbilityRequiresNamespace {
                ability,
                expected_namespace,
            } => {
                write!(
                    f,
                    "ability `{ability}` requires `{expected_namespace}.` prefix"
                )
            }
            Self::UndefinedTypeName { name } => {
                write!(f, "undefined type: `{name}`")
            }
            Self::NotARecordType { ty } => {
                write!(f, "type `{ty}` is not a record type and cannot be constructed with {{ field: value }} syntax")
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
        }
    }
}

//! Compilation error types.

use std::sync::Arc;

use crate::ast::BindingId;

/// An error that occurred during compilation.
#[derive(Debug, Clone)]
pub struct CompileError {
    /// The kind of error.
    pub kind: CompileErrorKind,
    /// Source location (byte offset range).
    pub span: (u32, u32),
}

impl CompileError {
    /// Create a new compile error.
    #[must_use]
    pub fn new(kind: CompileErrorKind, span: (u32, u32)) -> Self {
        Self { kind, span }
    }
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.kind)
    }
}

impl std::error::Error for CompileError {}

/// The kind of compilation error.
#[derive(Debug, Clone)]
pub enum CompileErrorKind {
    /// Undefined function reference.
    UndefinedFunction { name: Arc<str> },

    /// Undefined local variable.
    UndefinedLocal { id: BindingId },

    /// Too many local variables.
    TooManyLocals { count: usize },

    /// Too many constants.
    TooManyConstants { count: usize },

    /// Unsupported expression (not yet implemented).
    Unsupported { feature: String },

    /// Ability not registered.
    UnknownAbility { name: Arc<str> },

    /// Unknown ability method.
    UnknownAbilityMethod { ability: Arc<str>, method: Arc<str> },

    /// Internal compiler error (invariant violation).
    Internal { message: &'static str },
}

impl std::fmt::Display for CompileErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UndefinedFunction { name } => write!(f, "undefined function: `{name}`"),
            Self::UndefinedLocal { id } => write!(f, "undefined local variable: binding {id}"),
            Self::TooManyLocals { count } => {
                write!(f, "too many local variables: {count} (max 65535)")
            }
            Self::TooManyConstants { count } => {
                write!(f, "too many constants: {count} (max 65535)")
            }
            Self::Unsupported { feature } => write!(f, "unsupported feature: {feature}"),
            Self::UnknownAbility { name } => write!(f, "unknown ability: `{name}`"),
            Self::UnknownAbilityMethod { ability, method } => {
                write!(f, "unknown ability method: `{ability}.{method}`")
            }
            Self::Internal { message } => write!(f, "internal compiler error: {message}"),
        }
    }
}

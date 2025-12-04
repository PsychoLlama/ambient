//! Parse error types for the Ambient parser.

use std::fmt;

use ambient_engine::ast::Span;

/// A parsing error with source location.
#[derive(Debug, Clone)]
pub struct ParseError {
    /// The kind of error.
    pub kind: ParseErrorKind,
    /// Source location of the error.
    pub span: Span,
    /// Optional context message.
    pub context: Option<String>,
}

impl ParseError {
    /// Create a new parse error.
    #[must_use]
    pub fn new(kind: ParseErrorKind, span: Span) -> Self {
        Self {
            kind,
            span,
            context: None,
        }
    }

    /// Add context to an error.
    #[must_use]
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)?;
        if let Some(ctx) = &self.context {
            write!(f, " ({ctx})")?;
        }
        write!(f, " at {}..{}", self.span.start, self.span.end)?;
        Ok(())
    }
}

impl std::error::Error for ParseError {}

/// The kind of parse error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseErrorKind {
    // ─────────────────────────────────────────────────────────────────────────
    // Lexer errors
    // ─────────────────────────────────────────────────────────────────────────
    /// Unexpected character in input.
    UnexpectedChar(char),

    /// Unterminated string literal.
    UnterminatedString,

    /// Unterminated string interpolation.
    UnterminatedInterpolation,

    /// Invalid escape sequence in string.
    InvalidEscape(char),

    /// Invalid number literal.
    InvalidNumber(String),

    // ─────────────────────────────────────────────────────────────────────────
    // Parser errors
    // ─────────────────────────────────────────────────────────────────────────
    /// Expected a specific token but found something else.
    Expected { expected: String, found: String },

    /// Unexpected end of file.
    UnexpectedEof,

    /// Unexpected token.
    UnexpectedToken(String),

    /// Invalid pattern.
    InvalidPattern,

    /// Invalid type expression.
    InvalidType,

    /// Invalid expression.
    InvalidExpression,

    /// Duplicate field in record.
    DuplicateField(String),

    /// Duplicate parameter name.
    DuplicateParameter(String),

    /// Duplicate variant name.
    DuplicateVariant(String),

    /// Invalid ability syntax.
    InvalidAbilitySyntax(String),

    /// Too many items (exceeds parser limits).
    TooManyItems { kind: String, max: usize },

    // ─────────────────────────────────────────────────────────────────────────
    // Lowering errors
    // ─────────────────────────────────────────────────────────────────────────
    /// Error during CST to AST lowering.
    LoweringError(String),

    // ─────────────────────────────────────────────────────────────────────────
    // Name resolution errors
    // ─────────────────────────────────────────────────────────────────────────
    /// Undefined name.
    UndefinedName(String),

    /// Duplicate definition.
    DuplicateDefinition(String),
}

impl fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedChar(c) => write!(f, "unexpected character '{c}'"),
            Self::UnterminatedString => write!(f, "unterminated string literal"),
            Self::UnterminatedInterpolation => write!(f, "unterminated string interpolation"),
            Self::InvalidEscape(c) => write!(f, "invalid escape sequence '\\{c}'"),
            Self::InvalidNumber(s) => write!(f, "invalid number literal '{s}'"),

            Self::Expected { expected, found } => {
                write!(f, "expected {expected}, found {found}")
            }
            Self::UnexpectedEof => write!(f, "unexpected end of file"),
            Self::UnexpectedToken(t) => write!(f, "unexpected token '{t}'"),
            Self::InvalidPattern => write!(f, "invalid pattern"),
            Self::InvalidType => write!(f, "invalid type expression"),
            Self::InvalidExpression => write!(f, "invalid expression"),
            Self::DuplicateField(name) => write!(f, "duplicate field '{name}'"),
            Self::DuplicateParameter(name) => write!(f, "duplicate parameter '{name}'"),
            Self::DuplicateVariant(name) => write!(f, "duplicate variant '{name}'"),
            Self::InvalidAbilitySyntax(msg) => write!(f, "invalid ability syntax: {msg}"),
            Self::TooManyItems { kind, max } => {
                write!(f, "too many {kind} (maximum is {max})")
            }

            Self::LoweringError(msg) => write!(f, "lowering error: {msg}"),

            Self::UndefinedName(name) => write!(f, "undefined name '{name}'"),
            Self::DuplicateDefinition(name) => write!(f, "duplicate definition '{name}'"),
        }
    }
}

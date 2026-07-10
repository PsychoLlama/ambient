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

    /// Invalid UUID in a `unique(...)` struct or enum declaration.
    InvalidUuid(String),

    /// The `unique(...)` parentheses did not contain a canonical uppercase
    /// UUID literal.
    ExpectedUuid,

    /// An `enum` was declared without the mandatory `unique(<uuid>)` prefix.
    /// Every enum is nominal, so it must carry an identity.
    EnumRequiresUnique,

    /// A unit struct (`struct Foo;`) was declared without the mandatory
    /// `unique(<uuid>)` prefix. A fieldless type has no structure to identify it
    /// by, so it must carry a nominal identity.
    UnitStructRequiresUnique,

    /// A struct was declared with an empty brace body (`struct Foo {}`). A struct
    /// with braces must declare at least one field; a fieldless type must use the
    /// unit form `unique(<uuid>) struct Foo;`.
    EmptyStructBody,

    /// An `extern` struct was declared without the mandatory `unique(<uuid>)`
    /// prefix. An engine-provided type needs a stable nominal identity for the
    /// engine to refer to it by.
    ExternStructRequiresUnique,

    /// An `extern fn` was declared with a `with` clause. Extern fns are pure
    /// by construction — effectful host integration goes through abilities.
    ExternFnWithAbilities,

    /// An `extern fn` was declared without a return type. There is no body to
    /// infer from, so the full signature is mandatory.
    ExternFnRequiresReturnType,

    /// An `extern fn` parameter was declared without a type annotation. There
    /// is no body to infer from, so the full signature is mandatory.
    ExternFnParamRequiresType(String),

    /// An `ability` was declared without the mandatory `unique(<uuid>)`
    /// prefix. Abilities are nominal like enums: the uuid is the identity,
    /// so renames and moves never change it.
    AbilityRequiresUnique,

    /// A `trait` was declared without the mandatory `unique(<uuid>)` prefix.
    /// Traits are nominal like enums and abilities: the uuid is the identity
    /// that bounds, impls, and dispatch key off, so renames and moves never
    /// change it and same-named traits never collide.
    TraitRequiresUnique,

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
            Self::InvalidUuid(msg) => write!(f, "invalid UUID: {msg}"),
            Self::ExpectedUuid => write!(
                f,
                "expected an uppercase UUID literal (e.g. A1B2C3D4-0000-0000-0000-000000000001)"
            ),
            Self::EnumRequiresUnique => write!(
                f,
                "enum declarations require a `unique(<uuid>)` prefix \
                 (e.g. `unique(A1B2C3D4-0000-0000-0000-000000000001) enum Color {{ ... }}`)"
            ),
            Self::UnitStructRequiresUnique => write!(
                f,
                "unit structs require a `unique(<uuid>)` prefix \
                 (e.g. `unique(A1B2C3D4-0000-0000-0000-000000000001) struct Marker;`)"
            ),
            Self::EmptyStructBody => write!(
                f,
                "a struct with braces must declare at least one field; \
                 write a fieldless type as a unit struct \
                 (e.g. `unique(A1B2C3D4-0000-0000-0000-000000000001) struct Foo;`)"
            ),
            Self::ExternStructRequiresUnique => write!(
                f,
                "`extern` structs require a `unique(<uuid>)` prefix \
                 (e.g. `extern unique(A1B2C3D4-0000-0000-0000-000000000001) struct Foo;`)"
            ),
            Self::ExternFnWithAbilities => write!(
                f,
                "an `extern fn` cannot declare abilities — extern fns are pure; \
                 effectful host operations are declared as abilities instead"
            ),
            Self::ExternFnRequiresReturnType => write!(
                f,
                "an `extern fn` requires a declared return type \
                 (there is no body to infer it from)"
            ),
            Self::ExternFnParamRequiresType(name) => write!(
                f,
                "extern fn parameter `{name}` requires a type annotation \
                 (there is no body to infer it from)"
            ),
            Self::AbilityRequiresUnique => write!(
                f,
                "ability declarations require a `unique(<uuid>)` prefix \
                 (e.g. `unique(A1B2C3D4-0000-0000-0000-000000000001) ability Console {{ ... }}`)"
            ),
            Self::TraitRequiresUnique => write!(
                f,
                "trait declarations require a `unique(<uuid>)` prefix \
                 (e.g. `unique(A1B2C3D4-0000-0000-0000-000000000001) trait Show {{ ... }}`)"
            ),

            Self::UndefinedName(name) => write!(f, "undefined name '{name}'"),
            Self::DuplicateDefinition(name) => write!(f, "duplicate definition '{name}'"),
        }
    }
}

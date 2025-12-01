//! Concrete Syntax Tree (CST) for the Ambient language.
//!
//! The CST preserves all syntactic information including:
//! - Whitespace and comments (trivia)
//! - Source spans for every node
//! - Original token text
//!
//! This representation is suitable for:
//! - IDE tooling (code formatting, syntax highlighting)
//! - Error recovery (partial parsing)
//! - Source-to-source transformations
//!
//! The CST is lowered to the AST (from `ambient-engine`) for type checking
//! and compilation.

use std::sync::Arc;

use ambient_engine::ast::Span;

/// Trivia: whitespace and comments attached to tokens.
#[derive(Debug, Clone, Default)]
pub struct Trivia {
    /// Trivia items.
    pub items: Vec<TriviaItem>,
}

impl Trivia {
    /// Check if trivia is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Check if trivia contains any comments.
    #[must_use]
    pub fn has_comments(&self) -> bool {
        self.items.iter().any(|t| matches!(t.kind, TriviaKind::Comment))
    }
}

/// A single trivia item.
#[derive(Debug, Clone)]
pub struct TriviaItem {
    /// Kind of trivia.
    pub kind: TriviaKind,
    /// Source span.
    pub span: Span,
    /// Original text.
    pub text: Arc<str>,
}

/// Kind of trivia.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriviaKind {
    /// Whitespace (spaces, tabs, newlines).
    Whitespace,
    /// Line comment (`// ...`).
    Comment,
}

// ─────────────────────────────────────────────────────────────────────────────
// Module and Items
// ─────────────────────────────────────────────────────────────────────────────

/// A complete module (file) in the CST.
#[derive(Debug, Clone)]
pub struct CstModule {
    /// Module name (from file path).
    pub name: Arc<str>,
    /// Leading trivia before first item.
    pub leading_trivia: Trivia,
    /// Top-level items.
    pub items: Vec<CstItem>,
    /// Trailing trivia after last item.
    pub trailing_trivia: Trivia,
    /// Full source span.
    pub span: Span,
}

/// A top-level item in the CST.
#[derive(Debug, Clone)]
pub struct CstItem {
    /// Leading trivia.
    pub leading_trivia: Trivia,
    /// Item kind.
    pub kind: CstItemKind,
    /// Source span.
    pub span: Span,
}

/// The kind of top-level item.
#[derive(Debug, Clone)]
pub enum CstItemKind {
    /// Function definition.
    Function(CstFunctionDef),
    /// Constant definition.
    Const(CstConstDef),
    /// Type alias.
    TypeAlias(CstTypeAliasDef),
    /// Enum definition.
    Enum(CstEnumDef),
    /// Ability definition.
    Ability(CstAbilityDef),
    /// Use/import statement.
    Use(CstUseDef),
    /// Error recovery placeholder.
    Error,
}

// ─────────────────────────────────────────────────────────────────────────────
// Function Definition
// ─────────────────────────────────────────────────────────────────────────────

/// A function definition.
#[derive(Debug, Clone)]
pub struct CstFunctionDef {
    /// Whether public (`pub fn`).
    pub is_public: bool,
    /// Function name.
    pub name: CstIdent,
    /// Type parameters (generics).
    pub type_params: Vec<CstTypeParam>,
    /// Parameters.
    pub params: Vec<CstParam>,
    /// Return type.
    pub ret_ty: Option<CstTypeExpr>,
    /// Abilities used (`with Ability1, Ability2`).
    pub abilities: Vec<CstQualifiedName>,
    /// Function body.
    pub body: CstExpr,
}

/// An identifier with trivia.
#[derive(Debug, Clone)]
pub struct CstIdent {
    /// Identifier name.
    pub name: Arc<str>,
    /// Source span.
    pub span: Span,
    /// Trailing trivia.
    pub trailing_trivia: Trivia,
}

/// A type parameter (generic).
#[derive(Debug, Clone)]
pub struct CstTypeParam {
    /// Parameter name.
    pub name: CstIdent,
    /// Whether this is an ability variable (has `!` suffix in declaration).
    pub is_ability: bool,
    /// Source span.
    pub span: Span,
}

/// A function parameter.
#[derive(Debug, Clone)]
pub struct CstParam {
    /// Parameter name.
    pub name: CstIdent,
    /// Type annotation.
    pub ty: Option<CstTypeExpr>,
    /// Source span.
    pub span: Span,
}

/// A qualified name (potentially with module path).
#[derive(Debug, Clone)]
pub struct CstQualifiedName {
    /// Path segments.
    pub segments: Vec<CstIdent>,
    /// Source span covering the whole name.
    pub span: Span,
}

// ─────────────────────────────────────────────────────────────────────────────
// Type Expressions
// ─────────────────────────────────────────────────────────────────────────────

/// A type expression.
#[derive(Debug, Clone)]
pub struct CstTypeExpr {
    /// Type expression kind.
    pub kind: CstTypeExprKind,
    /// Source span.
    pub span: Span,
}

/// The kind of type expression.
#[derive(Debug, Clone)]
pub enum CstTypeExprKind {
    /// Named type (could be primitive or user-defined).
    Name(CstQualifiedName),

    /// Generic type application: `List<T>`.
    Generic {
        base: Box<CstTypeExpr>,
        args: Vec<CstTypeExpr>,
    },

    /// Tuple type: `(A, B, C)`.
    Tuple(Vec<CstTypeExpr>),

    /// Record type: `{ x: A, y: B }`.
    Record(Vec<(CstIdent, CstTypeExpr)>),

    /// Function type: `(A, B) -> C` or `(A) -> B with Ability`.
    Function {
        params: Vec<CstTypeExpr>,
        ret: Box<CstTypeExpr>,
        abilities: Vec<CstQualifiedName>,
    },

    /// Ability value type: `Ability<T, A!>`.
    AbilityValue {
        result_ty: Box<CstTypeExpr>,
        ability_ty: Box<CstTypeExpr>,
    },

    /// Never type: `!`.
    Never,

    /// Inferred type: `_`.
    Infer,

    /// Error recovery placeholder.
    Error,
}

// ─────────────────────────────────────────────────────────────────────────────
// Other Definitions
// ─────────────────────────────────────────────────────────────────────────────

/// A constant definition.
#[derive(Debug, Clone)]
pub struct CstConstDef {
    /// Constant name.
    pub name: CstIdent,
    /// Type annotation.
    pub ty: CstTypeExpr,
    /// Value expression.
    pub value: CstExpr,
}

/// A type alias definition.
#[derive(Debug, Clone)]
pub struct CstTypeAliasDef {
    /// Type name.
    pub name: CstIdent,
    /// Type parameters.
    pub type_params: Vec<CstTypeParam>,
    /// The aliased type.
    pub ty: CstTypeExpr,
    /// Optional unique UUID for nominal types.
    pub unique_id: Option<Arc<str>>,
}

/// An enum definition.
#[derive(Debug, Clone)]
pub struct CstEnumDef {
    /// Enum name.
    pub name: CstIdent,
    /// Type parameters.
    pub type_params: Vec<CstTypeParam>,
    /// Variants.
    pub variants: Vec<CstEnumVariant>,
}

/// An enum variant.
#[derive(Debug, Clone)]
pub struct CstEnumVariant {
    /// Variant name.
    pub name: CstIdent,
    /// Optional payload type.
    pub payload: Option<CstTypeExpr>,
    /// Source span.
    pub span: Span,
}

/// An ability definition.
#[derive(Debug, Clone)]
pub struct CstAbilityDef {
    /// Ability name.
    pub name: CstIdent,
    /// Dependencies (`with OtherAbility`).
    pub dependencies: Vec<CstQualifiedName>,
    /// Methods.
    pub methods: Vec<CstAbilityMethod>,
}

/// An ability method signature.
#[derive(Debug, Clone)]
pub struct CstAbilityMethod {
    /// Method name.
    pub name: CstIdent,
    /// Type parameters.
    pub type_params: Vec<CstTypeParam>,
    /// Parameters.
    pub params: Vec<(CstIdent, CstTypeExpr)>,
    /// Return type.
    pub ret_ty: CstTypeExpr,
    /// Source span.
    pub span: Span,
}

/// A use/import definition.
#[derive(Debug, Clone)]
pub struct CstUseDef {
    /// Import path.
    pub path: Vec<CstIdent>,
    /// What to import.
    pub imports: CstUseImports,
    /// Source span.
    pub span: Span,
}

/// What to import from a use path.
#[derive(Debug, Clone)]
pub enum CstUseImports {
    /// Import everything: `use module.*`.
    All,
    /// Import specific items: `use module.{a, b}`.
    Items(Vec<CstIdent>),
    /// Import single item (path ends with the item).
    Single,
}

// ─────────────────────────────────────────────────────────────────────────────
// Expressions
// ─────────────────────────────────────────────────────────────────────────────

/// An expression.
#[derive(Debug, Clone)]
pub struct CstExpr {
    /// Expression kind.
    pub kind: CstExprKind,
    /// Source span.
    pub span: Span,
}

/// The kind of expression.
#[derive(Debug, Clone)]
pub enum CstExprKind {
    // ─────────────────────────────────────────────────────────────────────────
    // Literals
    // ─────────────────────────────────────────────────────────────────────────
    /// Unit literal: `()`.
    Unit,

    /// Boolean literal.
    Bool(bool),

    /// Number literal.
    Number(f64),

    /// String literal (no interpolation).
    String(Arc<str>),

    /// Interpolated string: parts alternate between string and expression.
    InterpolatedString(Vec<StringPart>),

    // ─────────────────────────────────────────────────────────────────────────
    // Identifiers and references
    // ─────────────────────────────────────────────────────────────────────────
    /// Identifier reference.
    Ident(CstIdent),

    /// Qualified name: `Module.function`.
    QualifiedName(CstQualifiedName),

    // ─────────────────────────────────────────────────────────────────────────
    // Compound expressions
    // ─────────────────────────────────────────────────────────────────────────
    /// Tuple: `(a, b, c)`.
    Tuple(Vec<CstExpr>),

    /// Tuple index: `tuple.0`.
    TupleIndex {
        tuple: Box<CstExpr>,
        index: u32,
    },

    /// Record: `{ x: 1, y: 2 }`.
    Record(Vec<(CstIdent, CstExpr)>),

    /// Record field access: `record.field`.
    Field {
        record: Box<CstExpr>,
        field: CstIdent,
    },

    /// List: `[a, b, c]`.
    List(Vec<CstExpr>),

    // ─────────────────────────────────────────────────────────────────────────
    // Operators
    // ─────────────────────────────────────────────────────────────────────────
    /// Binary operation.
    Binary {
        op: CstBinaryOp,
        left: Box<CstExpr>,
        right: Box<CstExpr>,
    },

    /// Unary operation.
    Unary {
        op: CstUnaryOp,
        operand: Box<CstExpr>,
    },

    // ─────────────────────────────────────────────────────────────────────────
    // Control flow
    // ─────────────────────────────────────────────────────────────────────────
    /// If expression.
    If {
        condition: Box<CstExpr>,
        then_branch: Box<CstExpr>,
        else_branch: Option<Box<CstExpr>>,
    },

    /// Match expression.
    Match {
        scrutinee: Box<CstExpr>,
        arms: Vec<CstMatchArm>,
    },

    /// Block expression.
    Block {
        stmts: Vec<CstStmt>,
        result: Option<Box<CstExpr>>,
    },

    // ─────────────────────────────────────────────────────────────────────────
    // Functions and calls
    // ─────────────────────────────────────────────────────────────────────────
    /// Lambda: `(x, y) => x + y`.
    Lambda(CstLambda),

    /// Function call: `f(a, b)`.
    Call {
        callee: Box<CstExpr>,
        args: Vec<CstExpr>,
    },

    // ─────────────────────────────────────────────────────────────────────────
    // Abilities
    // ─────────────────────────────────────────────────────────────────────────
    /// Ability call with perform: `Console.print!("hello")`.
    Perform {
        ability: CstQualifiedName,
        method: CstIdent,
        args: Vec<CstExpr>,
    },

    /// Suspended ability: `Console.print("hello")`.
    Suspend {
        ability: CstQualifiedName,
        method: CstIdent,
        args: Vec<CstExpr>,
    },

    /// Handle expression.
    Handle(Box<CstHandleExpr>),

    /// Resume a continuation with a value: `resume(value)`.
    Resume(Box<CstExpr>),

    /// Handler literal: `{ read(path) => resume("content"), ... }`.
    /// Creates a first-class handler value.
    HandlerLiteral(Box<CstHandlerLiteralExpr>),

    // ─────────────────────────────────────────────────────────────────────────
    // Error recovery
    // ─────────────────────────────────────────────────────────────────────────
    /// Error placeholder.
    Error,
}

/// A part of an interpolated string.
#[derive(Debug, Clone)]
pub enum StringPart {
    /// Literal text.
    Literal(Arc<str>, Span),
    /// Interpolated expression.
    Expr(CstExpr),
}

/// Binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CstBinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

/// Unary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CstUnaryOp {
    Neg,
    Not,
}

/// A lambda expression.
#[derive(Debug, Clone)]
pub struct CstLambda {
    /// Parameters.
    pub params: Vec<CstParam>,
    /// Return type.
    pub ret_ty: Option<CstTypeExpr>,
    /// Body.
    pub body: Box<CstExpr>,
    /// Source span.
    pub span: Span,
}

/// A match arm.
#[derive(Debug, Clone)]
pub struct CstMatchArm {
    /// Pattern.
    pub pattern: CstPattern,
    /// Optional guard.
    pub guard: Option<CstExpr>,
    /// Body expression.
    pub body: CstExpr,
    /// Source span.
    pub span: Span,
}

/// A handle expression.
#[derive(Debug, Clone)]
pub struct CstHandleExpr {
    /// Body being handled.
    pub body: CstExpr,
    /// Handlers.
    pub handlers: Vec<CstHandler>,
    /// Else clause for normal return.
    pub else_clause: Option<CstExpr>,
    /// Source span.
    pub span: Span,
}

/// A handler for an ability method.
#[derive(Debug, Clone)]
pub struct CstHandler {
    /// Ability being handled.
    pub ability: CstQualifiedName,
    /// Method being handled.
    pub method: CstIdent,
    /// Parameters.
    pub params: Vec<CstParam>,
    /// Handler body.
    pub body: CstExpr,
    /// Source span.
    pub span: Span,
}

/// A handler literal expression creating a first-class handler value.
///
/// Syntax: `{ method(params) => body, ... }`
///
/// Example:
/// ```ambient
/// let mock_fs: Handler<Filesystem> = {
///   read(path) => resume("mock content"),
///   write(path, content) => resume(()),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct CstHandlerLiteralExpr {
    /// The handler methods.
    pub methods: Vec<CstHandlerLiteralMethod>,
    /// Source span.
    pub span: Span,
}

/// A method in a handler literal.
#[derive(Debug, Clone)]
pub struct CstHandlerLiteralMethod {
    /// Method name.
    pub method: CstIdent,
    /// Parameters.
    pub params: Vec<CstParam>,
    /// Handler body.
    pub body: CstExpr,
    /// Source span.
    pub span: Span,
}

// ─────────────────────────────────────────────────────────────────────────────
// Patterns
// ─────────────────────────────────────────────────────────────────────────────

/// A pattern for destructuring.
#[derive(Debug, Clone)]
pub struct CstPattern {
    /// Pattern kind.
    pub kind: CstPatternKind,
    /// Source span.
    pub span: Span,
}

/// The kind of pattern.
#[derive(Debug, Clone)]
pub enum CstPatternKind {
    /// Wildcard: `_`.
    Wildcard,

    /// Variable binding: `x`.
    Binding(CstIdent),

    /// Literal: `42`, `"hello"`, `true`.
    Literal(CstLiteral),

    /// Tuple: `(a, b, c)`.
    Tuple(Vec<CstPattern>),

    /// Record: `{ x, y: renamed }`.
    Record(Vec<CstRecordPatternField>),

    /// Enum variant: `Some(x)`, `None`.
    Variant {
        name: CstQualifiedName,
        payload: Option<Box<CstPattern>>,
    },

    /// Error recovery.
    Error,
}

/// A literal value in a pattern.
#[derive(Debug, Clone)]
pub enum CstLiteral {
    Unit,
    Bool(bool),
    Number(f64),
    String(Arc<str>),
}

/// A field in a record pattern.
#[derive(Debug, Clone)]
pub struct CstRecordPatternField {
    /// Field name.
    pub field: CstIdent,
    /// Pattern (if different from field name).
    pub pattern: Option<CstPattern>,
    /// Source span.
    pub span: Span,
}

// ─────────────────────────────────────────────────────────────────────────────
// Statements
// ─────────────────────────────────────────────────────────────────────────────

/// A statement.
#[derive(Debug, Clone)]
pub struct CstStmt {
    /// Leading trivia.
    pub leading_trivia: Trivia,
    /// Statement kind.
    pub kind: CstStmtKind,
    /// Source span.
    pub span: Span,
}

/// The kind of statement.
#[derive(Debug, Clone)]
pub enum CstStmtKind {
    /// Let binding.
    Let(CstLetBinding),
    /// Expression statement.
    Expr(CstExpr),
    /// Error recovery.
    Error,
}

/// A let binding.
#[derive(Debug, Clone)]
pub struct CstLetBinding {
    /// Variable name.
    pub name: CstIdent,
    /// Optional type annotation.
    pub ty: Option<CstTypeExpr>,
    /// Initializer expression.
    pub init: CstExpr,
}

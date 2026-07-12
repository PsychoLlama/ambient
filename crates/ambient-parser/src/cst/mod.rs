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

mod traits;
pub use traits::*;

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
        self.items
            .iter()
            .any(|t| matches!(t.kind, TriviaKind::Comment))
    }

    /// Extract doc comments (`///`) as a single markdown string.
    /// Adjacent doc comments are joined with newlines.
    #[must_use]
    pub fn extract_doc_comments(&self) -> Option<String> {
        let docs: Vec<&str> = self
            .items
            .iter()
            .filter(|item| item.kind == TriviaKind::DocComment)
            .map(|item| {
                // Strip `/// ` or `///` prefix
                let text = item.text.as_ref();
                text.strip_prefix("/// ")
                    .or_else(|| text.strip_prefix("///"))
                    .unwrap_or(text)
            })
            .collect();

        if docs.is_empty() {
            None
        } else {
            Some(docs.join("\n"))
        }
    }

    /// Extract inner doc comments (`//!`) as a single markdown string.
    /// Adjacent inner doc comments are joined with newlines.
    #[must_use]
    pub fn extract_inner_doc_comments(&self) -> Option<String> {
        let docs: Vec<&str> = self
            .items
            .iter()
            .filter(|item| item.kind == TriviaKind::InnerDocComment)
            .map(|item| {
                // Strip `//! ` or `//!` prefix
                let text = item.text.as_ref();
                text.strip_prefix("//! ")
                    .or_else(|| text.strip_prefix("//!"))
                    .unwrap_or(text)
            })
            .collect();

        if docs.is_empty() {
            None
        } else {
            Some(docs.join("\n"))
        }
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
    /// Doc comment (`/// ...`).
    DocComment,
    /// Inner doc comment (`//! ...`).
    InnerDocComment,
}

// ─────────────────────────────────────────────────────────────────────────────
// REPL Input
// ─────────────────────────────────────────────────────────────────────────────

/// Input to the REPL, which may be either an item definition or an expression.
#[derive(Debug, Clone)]
pub enum CstReplInput {
    /// An item definition (function, const, type, etc.).
    Item(Box<CstItem>),
    /// An expression to evaluate.
    Expr(CstExpr),
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
    /// Struct (record) definition.
    Struct(CstStructDef),
    /// Type alias.
    TypeAlias(CstTypeAliasDef),
    /// Enum definition.
    Enum(CstEnumDef),
    /// Ability definition.
    Ability(CstAbilityDef),
    /// Use/import statement.
    Use(CstUseDef),
    /// Trait definition.
    Trait(CstTraitDef),
    /// Trait implementation.
    Impl(CstImplDef),
    /// Extern function declaration (`extern fn name(...): Ret;`).
    ExternFn(CstExternFnDef),
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

/// An `extern fn` declaration: a body-less function signature terminated by
/// `;`, whose implementation is bound by the host. Parameters and the return
/// type are syntactically optional (shared parsing paths) — lowering enforces
/// that both are fully declared, since there is no body to infer from.
#[derive(Debug, Clone)]
pub struct CstExternFnDef {
    /// Whether public (`pub extern fn`).
    pub is_public: bool,
    /// Function name.
    pub name: CstIdent,
    /// Type parameters (generics).
    pub type_params: Vec<CstTypeParam>,
    /// Parameters.
    pub params: Vec<CstParam>,
    /// Return type.
    pub ret_ty: Option<CstTypeExpr>,
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
    /// Trait bounds (`T: Eq + Ord`). Empty for an unbounded parameter.
    pub bounds: Vec<CstQualifiedName>,
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

    /// Handler value type: `Handler<A>` / `Handler<A, R>`. `Handler` is type
    /// syntax, not a name: `ability` is an ability reference (resolved
    /// through the ability namespace), `answer` the optional answer type `R`.
    Handler {
        ability: CstQualifiedName,
        answer: Option<Box<CstTypeExpr>>,
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
    /// Whether public (`pub const`).
    pub is_public: bool,
    /// Constant name.
    pub name: CstIdent,
    /// Type annotation, or `None` when omitted (inferred from the literal).
    pub ty: Option<CstTypeExpr>,
    /// Value expression.
    pub value: CstExpr,
}

/// A struct (record) definition: `struct Foo { fields }`, optionally prefixed
/// with `unique(<uuid>)` for a nominal identity. There is no `= Type` form
/// (that is a [`CstTypeAliasDef`]). The body is a record type, or `None` for the
/// unit form `struct Foo;` — a fieldless nominal type.
#[derive(Debug, Clone)]
pub struct CstStructDef {
    /// Whether public (`pub struct`).
    pub is_public: bool,
    /// Struct name.
    pub name: CstIdent,
    /// Type parameters.
    pub type_params: Vec<CstTypeParam>,
    /// The record body, or `None` for a unit struct (`struct Foo;`).
    pub ty: Option<CstTypeExpr>,
    /// Optional unique UUID for a nominal identity (`unique(<uuid>)`).
    pub unique_id: Option<Arc<str>>,
    /// Whether this struct is `extern`: an engine-provided type that user code
    /// may name and read from but not construct. Requires `unique(...)`.
    pub is_extern: bool,
}

/// A type alias definition: `type Foo = Bar`.
#[derive(Debug, Clone)]
pub struct CstTypeAliasDef {
    /// Whether public (`pub type`).
    pub is_public: bool,
    /// Type name.
    pub name: CstIdent,
    /// Type parameters.
    pub type_params: Vec<CstTypeParam>,
    /// The aliased type.
    pub ty: CstTypeExpr,
}

/// An enum definition.
#[derive(Debug, Clone)]
pub struct CstEnumDef {
    /// Whether public (`pub enum`).
    pub is_public: bool,
    /// Enum name.
    pub name: CstIdent,
    /// Type parameters.
    pub type_params: Vec<CstTypeParam>,
    /// Variants.
    pub variants: Vec<CstEnumVariant>,
    /// The `unique(<uuid>)` prefix's UUID text. Mandatory in a valid program
    /// — lowering rejects `None` — but kept optional in the CST so a bare
    /// `enum` still parses for error recovery and editor tooling.
    pub unique_id: Option<Arc<str>>,
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
    /// Whether public (`pub ability`).
    pub is_public: bool,
    /// Ability name.
    pub name: CstIdent,
    /// Dependencies (`with OtherAbility`).
    pub dependencies: Vec<CstQualifiedName>,
    /// Methods.
    pub methods: Vec<CstAbilityMethod>,
    /// Nominal identity from the `unique(<uuid>)` prefix. Abilities are
    /// nominal like enums; lowering rejects a missing prefix.
    pub unique_id: Option<Arc<str>>,
}

/// An ability method: a signature plus its default implementation (the body
/// that runs when a perform reaches no handler). The body is `None` only
/// during error recovery or for the `Exception` carve-out; the checker
/// rejects a missing body everywhere else.
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
    /// Default implementation body (`None` for a `;`-terminated signature).
    pub body: Option<CstExpr>,
    /// Source span.
    pub span: Span,
}

/// A use/import definition: a Rust-style use-tree.
///
/// Examples:
/// - `use pkg::utils;`
/// - `use pkg::utils::helper;`
/// - `use pkg::utils::{helper, deep::{a, b}};`
/// - `use core::primitives::number::sqrt as root2;`
/// - `use {core::primitives::number, core::system::Stdio};`
/// - `use self::sibling;` / `use super::parent;`
/// - `use utils::inner;` (rooted at a module alias from another `use`)
/// - `pub use pkg::other::Thing;`
#[derive(Debug, Clone)]
pub struct CstUseDef {
    /// Whether this is a public re-export (`pub use`).
    pub is_public: bool,
    /// The use tree.
    pub tree: CstUseTree,
    /// Source span.
    pub span: Span,
}

/// One node of a use tree: leading path segments and a tail.
///
/// Keyword roots (`pkg`, `core`, `self`, `super`, contextual `platform`)
/// are ordinary segments here; lowering validates they only appear at the
/// head of a full path.
#[derive(Debug, Clone)]
pub struct CstUseTree {
    /// Leading path segments (empty for a root brace group).
    pub segments: Vec<CstIdent>,
    /// What follows the path.
    pub kind: CstUseTreeKind,
    /// Source span.
    pub span: Span,
}

/// The tail of a use-tree node.
#[derive(Debug, Clone)]
pub enum CstUseTreeKind {
    /// A leaf: import the final path segment, optionally renamed with
    /// `as`.
    Leaf {
        /// The `as` alias, if given.
        alias: Option<CstIdent>,
    },
    /// A brace group: `path::{tree, tree}` (or `{tree, tree}` at the
    /// root).
    Group(Vec<CstUseTree>),
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
    TupleIndex { tuple: Box<CstExpr>, index: u32 },

    /// Record: `{ x: 1, y: 2 }`.
    Record(Vec<(CstIdent, CstExpr)>),

    /// Typed record construction: `TypeName { x: 1, y: 2 }`.
    TypedRecord {
        type_name: CstQualifiedName,
        fields: Vec<(CstIdent, CstExpr)>,
    },

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

    /// Method call: `receiver.method(args)`.
    MethodCall {
        receiver: Box<CstExpr>,
        method: CstIdent,
        args: Vec<CstExpr>,
    },

    // ─────────────────────────────────────────────────────────────────────────
    // Abilities
    // ─────────────────────────────────────────────────────────────────────────
    /// Ability call with perform: `Stdio.out!("hello")`.
    Perform {
        ability: CstQualifiedName,
        method: CstIdent,
        args: Vec<CstExpr>,
    },

    /// Handle expression: `with H₁, …, Hₙ handle BODY [else E]`.
    Handle(Box<CstWithHandleExpr>),

    /// Resume a continuation with a value: `resume(value)`.
    Resume(Box<CstExpr>),

    /// Handler literal: `{ read(path) => resume("content"), ... }`.
    /// Creates a first-class handler value.
    HandlerLiteral(Box<CstHandlerLiteralExpr>),

    /// Sandbox expression: `sandbox with Ability { ... }` or `sandbox { ... }`.
    /// Restricts available abilities in the body.
    Sandbox(Box<CstSandboxExpr>),

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

/// A handle expression: `with H₁, …, Hₙ handle BODY [else E]`.
#[derive(Debug, Clone)]
pub struct CstWithHandleExpr {
    /// The handler expressions in the `with` list, in source order. Each is
    /// an ordinary expression (a handler literal or a `Handler<A, R>` value).
    pub handlers: Vec<CstExpr>,
    /// Body being handled.
    pub body: CstExpr,
    /// Else clause for normal return: `else EXPR`.
    pub else_clause: Option<CstExpr>,
    /// Source span.
    pub span: Span,
}

/// A handler literal expression creating a first-class handler value.
///
/// Syntax: `{ Ability::method(params) => body, ... }`
///
/// Example:
/// ```ambient
/// let mock_fs: Handler<FileSystem> = {
///   FileSystem::read(path) => resume("mock content"),
///   FileSystem::write(path, content) => resume(()),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct CstHandlerLiteralExpr {
    /// The handler methods.
    pub methods: Vec<CstHandlerLiteralMethod>,
    /// Source span.
    pub span: Span,
}

/// A method in a handler literal: `Ability::method(params) => body`.
#[derive(Debug, Clone)]
pub struct CstHandlerLiteralMethod {
    /// The ability the method belongs to, written as a qualified prefix.
    pub ability: CstQualifiedName,
    /// Method name.
    pub method: CstIdent,
    /// Parameters.
    pub params: Vec<CstParam>,
    /// Handler body.
    pub body: CstExpr,
    /// Source span.
    pub span: Span,
}

/// A sandbox expression for restricting available abilities.
///
/// Syntax: `sandbox with Ability1, Ability2 { body }` or `sandbox { body }`
///
/// Example:
/// ```ambient
/// sandbox with Log {
///   // Only Log ability available here
///   untrusted_code()
/// }
///
/// sandbox {
///   // No abilities available - pure computation only
///   pure_untrusted_code()
/// }
/// ```
#[derive(Debug, Clone)]
pub struct CstSandboxExpr {
    /// Allowed abilities (empty means pure computation only).
    pub allowed_abilities: Vec<CstQualifiedName>,
    /// Body expression to run in sandboxed context.
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
    /// Block-scoped import: `use pkg::utils::helper;`.
    Use(CstUseDef),
    /// Block-scoped constant: `const NAME = <literal>;`.
    Const(CstConstDef),
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

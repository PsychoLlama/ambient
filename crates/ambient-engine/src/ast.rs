//! Abstract Syntax Tree (AST) representation for the Ambient language.
//!
//! This module defines the AST used for:
//! - Type checking and inference
//! - Compilation to bytecode
//!
//! The AST is designed to be:
//! - Fully typed after type inference
//! - Serializable for debugging and tooling
//! - Easy to traverse and transform

use std::sync::Arc;

use uuid::Uuid;

use crate::types::Type;

/// A source location span for error reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    /// Start byte offset in the source.
    pub start: u32,
    /// End byte offset in the source (exclusive).
    pub end: u32,
}

impl Span {
    /// Create a new span.
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// Create a span covering two spans.
    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        Self {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

/// A node with source location information.
#[derive(Debug, Clone)]
pub struct Spanned<T> {
    /// The underlying value.
    pub node: T,
    /// Source location.
    pub span: Span,
}

impl<T> Spanned<T> {
    /// Create a new spanned value.
    #[must_use]
    pub const fn new(node: T, span: Span) -> Self {
        Self { node, span }
    }

    /// Map over the inner value.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Spanned<U> {
        Spanned {
            node: f(self.node),
            span: self.span,
        }
    }
}

/// A unique identifier for a local binding (parameter or let binding).
pub type BindingId = u32;

/// A reference to a named item (function, type, ability).
#[derive(Debug, Clone)]
pub struct QualifiedName {
    /// Module path segments (empty for local names).
    pub path: Vec<Arc<str>>,
    /// Source spans for each path segment (for IDE features).
    /// Same length as `path`, or empty if spans are not available.
    pub path_spans: Vec<Span>,
    /// The final name.
    pub name: Arc<str>,
    /// Source span for the name (for IDE features).
    pub name_span: Option<Span>,
}

impl PartialEq for QualifiedName {
    fn eq(&self, other: &Self) -> bool {
        // Ignore spans for equality - only compare semantic content
        self.path == other.path && self.name == other.name
    }
}

impl Eq for QualifiedName {}

impl QualifiedName {
    /// The full dotted form of this name (`core.List.map`), or just the
    /// name when the path is empty.
    #[must_use]
    pub fn joined(&self) -> Arc<str> {
        if self.path.is_empty() {
            return Arc::clone(&self.name);
        }
        let mut s = String::new();
        for segment in &self.path {
            s.push_str(segment);
            s.push('.');
        }
        s.push_str(&self.name);
        s.into()
    }

    /// Create a simple unqualified name.
    #[must_use]
    pub fn simple(name: impl Into<Arc<str>>) -> Self {
        Self {
            path: Vec::new(),
            path_spans: Vec::new(),
            name: name.into(),
            name_span: None,
        }
    }

    /// Create a qualified name with path.
    #[must_use]
    pub fn qualified(path: Vec<impl Into<Arc<str>>>, name: impl Into<Arc<str>>) -> Self {
        Self {
            path: path.into_iter().map(Into::into).collect(),
            path_spans: Vec::new(),
            name: name.into(),
            name_span: None,
        }
    }

    /// Create a qualified name with full span information.
    #[must_use]
    pub fn with_spans(
        path: Vec<Arc<str>>,
        path_spans: Vec<Span>,
        name: Arc<str>,
        name_span: Span,
    ) -> Self {
        Self {
            path,
            path_spans,
            name,
            name_span: Some(name_span),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Expressions
// ─────────────────────────────────────────────────────────────────────────────

/// An expression in the AST.
#[derive(Debug, Clone)]
pub struct Expr {
    /// The expression kind.
    pub kind: ExprKind,
    /// Source location.
    pub span: Span,
    /// Inferred type (filled in during type checking).
    pub ty: Option<Type>,
}

impl Expr {
    /// Create a new expression without type annotation.
    #[must_use]
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Self {
            kind,
            span,
            ty: None,
        }
    }

    /// Create a new expression with type annotation.
    #[must_use]
    pub fn typed(kind: ExprKind, span: Span, ty: Type) -> Self {
        Self {
            kind,
            span,
            ty: Some(ty),
        }
    }
}

/// The kind of expression.
#[derive(Debug, Clone)]
pub enum ExprKind {
    // ─────────────────────────────────────────────────────────────────────────
    // Literals
    // ─────────────────────────────────────────────────────────────────────────
    /// Unit literal `()`.
    Unit,

    /// Boolean literal.
    Bool(bool),

    /// Number literal (f64).
    Number(f64),

    /// String literal.
    String(Arc<str>),

    // ─────────────────────────────────────────────────────────────────────────
    // Variables and references
    // ─────────────────────────────────────────────────────────────────────────
    /// Local variable reference (parameter or let binding).
    Local(BindingId),

    /// Reference to a named item (function, constant).
    Name(QualifiedName),

    // ─────────────────────────────────────────────────────────────────────────
    // Compound expressions
    // ─────────────────────────────────────────────────────────────────────────
    /// Tuple construction: `(a, b, c)`.
    Tuple(Vec<Expr>),

    /// Tuple field access: `tuple.0`.
    TupleIndex(Box<Expr>, u32),

    /// Record construction: `{ x: 1, y: 2 }`.
    Record(Vec<(Arc<str>, Expr)>),

    /// Typed record construction: `TypeName { x: 1, y: 2 }`.
    TypedRecord {
        type_name: QualifiedName,
        fields: Vec<(Arc<str>, Expr)>,
    },

    /// Record field access: `record.field`.
    RecordField(Box<Expr>, Arc<str>),

    /// Method call: `receiver.method(args)`.
    /// Resolved to a trait method at type checking time.
    MethodCall {
        receiver: Box<Expr>,
        method: Arc<str>,
        method_span: Span,
        args: Vec<Expr>,
        /// Canonical impl-method symbol (filled in during type checking).
        /// The compiler resolves this like an ordinary function name.
        resolved_method: Option<Arc<str>>,
    },

    /// List literal: `[a, b, c]`.
    List(Vec<Expr>),

    // ─────────────────────────────────────────────────────────────────────────
    // Operators
    // ─────────────────────────────────────────────────────────────────────────
    /// Binary operation: `a + b`, `a && b`, etc.
    /// For primitive types, uses built-in operators.
    /// For nominal types, resolves to trait method (Add, Eq, etc.).
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        /// Canonical impl-method symbol for overloaded operators (filled during
        /// type checking). The compiler resolves this like an ordinary function name.
        resolved_op: Option<Arc<str>>,
    },

    /// Unary operation: `-x`, `!x`.
    Unary(UnaryOp, Box<Expr>),

    // ─────────────────────────────────────────────────────────────────────────
    // Control flow
    // ─────────────────────────────────────────────────────────────────────────
    /// If expression: `if cond { then } else { else }`.
    If(Box<Expr>, Box<Expr>, Option<Box<Expr>>),

    /// Match expression: `match expr { patterns }`.
    Match(Box<Expr>, Vec<MatchArm>),

    /// Block expression: `{ stmt1; stmt2; expr }`.
    Block(Vec<Stmt>, Option<Box<Expr>>),

    // ─────────────────────────────────────────────────────────────────────────
    // Functions and calls
    // ─────────────────────────────────────────────────────────────────────────
    /// Lambda expression: `(x, y) => x + y`.
    Lambda(Lambda),

    /// Function call: `f(a, b)`.
    Call(Box<Expr>, Vec<Expr>),

    // ─────────────────────────────────────────────────────────────────────────
    // Abilities
    // ─────────────────────────────────────────────────────────────────────────
    /// Ability method call with explicit perform: `Stdio.out!("hello")`.
    Perform(AbilityCall),

    /// Handle expression: `handle expr { handlers }`.
    Handle(HandleExpr),

    /// Resume a continuation with a value: `resume(value)`.
    Resume(Box<Expr>),

    /// Handler literal: `{ method(params) => body, ... }`.
    /// Creates a first-class handler value.
    HandlerLiteral(HandlerLiteralExpr),

    /// Sandbox expression: `sandbox with Ability { body }`.
    /// Restricts available abilities within the body.
    Sandbox(SandboxExpr),
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,

    // Comparison
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,

    // Logical
    And,
    Or,
}

impl BinaryOp {
    /// Returns true if this is a comparison operator.
    #[must_use]
    pub const fn is_comparison(self) -> bool {
        matches!(
            self,
            Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge
        )
    }

    /// Returns true if this is a logical operator.
    #[must_use]
    pub const fn is_logical(self) -> bool {
        matches!(self, Self::And | Self::Or)
    }

    /// Returns true if this is an arithmetic operator.
    #[must_use]
    pub const fn is_arithmetic(self) -> bool {
        matches!(
            self,
            Self::Add | Self::Sub | Self::Mul | Self::Div | Self::Mod
        )
    }
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// Numeric negation: `-x`.
    Neg,
    /// Logical not: `!x`.
    Not,
}

/// A match arm: `pattern => body`.
#[derive(Debug, Clone)]
pub struct MatchArm {
    /// The pattern to match.
    pub pattern: Pattern,
    /// Optional guard: `if condition`.
    pub guard: Option<Expr>,
    /// The arm body.
    pub body: Expr,
}

impl MatchArm {
    /// Create a new match arm without a guard.
    #[must_use]
    pub fn new(pattern: Pattern, body: Expr) -> Self {
        Self {
            pattern,
            guard: None,
            body,
        }
    }

    /// Create a new match arm with a guard.
    #[must_use]
    pub fn with_guard(pattern: Pattern, guard: Expr, body: Expr) -> Self {
        Self {
            pattern,
            guard: Some(guard),
            body,
        }
    }
}

/// A pattern for destructuring.
#[derive(Debug, Clone)]
pub struct Pattern {
    /// The pattern kind.
    pub kind: PatternKind,
    /// Source location.
    pub span: Span,
}

impl Pattern {
    /// Create a new pattern.
    #[must_use]
    pub const fn new(kind: PatternKind, span: Span) -> Self {
        Self { kind, span }
    }

    /// Create a wildcard pattern.
    #[must_use]
    pub fn wildcard() -> Self {
        Self::new(PatternKind::Wildcard, Span::default())
    }

    /// Create a binding pattern.
    #[must_use]
    pub fn binding(id: BindingId, name: impl Into<Arc<str>>) -> Self {
        Self::new(PatternKind::Binding(id, name.into()), Span::default())
    }

    /// Create a variant pattern with an optional inner pattern.
    #[must_use]
    pub fn variant(name: impl Into<Arc<str>>, inner: Option<Pattern>) -> Self {
        Self::new(
            PatternKind::Variant(QualifiedName::simple(name), inner.map(Box::new)),
            Span::default(),
        )
    }

    /// Create a literal pattern.
    #[must_use]
    pub fn literal(lit: Literal) -> Self {
        Self::new(PatternKind::Literal(lit), Span::default())
    }
}

/// The kind of pattern.
#[derive(Debug, Clone)]
pub enum PatternKind {
    /// Wildcard pattern: `_`.
    Wildcard,

    /// Variable binding: `x`.
    Binding(BindingId, Arc<str>),

    /// Literal pattern: `42`, `"hello"`, `true`.
    Literal(Literal),

    /// Tuple pattern: `(a, b, c)`.
    Tuple(Vec<Pattern>),

    /// Record pattern: `{ x, y: renamed }`.
    Record(Vec<(Arc<str>, Pattern)>),

    /// Enum variant pattern: `Some(x)`, `None`.
    Variant(QualifiedName, Option<Box<Pattern>>),
}

/// A literal value (used in patterns).
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Unit,
    Bool(bool),
    Number(f64),
    String(Arc<str>),
}

/// A lambda expression.
#[derive(Debug, Clone)]
pub struct Lambda {
    /// Parameters with optional type annotations.
    pub params: Vec<Param>,
    /// Optional return type annotation.
    pub ret_ty: Option<Type>,
    /// The body expression.
    pub body: Box<Expr>,
}

/// A function parameter.
#[derive(Debug, Clone)]
pub struct Param {
    /// Unique binding ID for this parameter.
    pub id: BindingId,
    /// Parameter name.
    pub name: Arc<str>,
    /// Optional type annotation.
    pub ty: Option<Type>,
    /// Source location.
    pub span: Span,
}

/// An ability method call.
#[derive(Debug, Clone)]
pub struct AbilityCall {
    /// The ability name.
    pub ability: QualifiedName,
    /// The method name.
    pub method: Arc<str>,
    /// Arguments.
    pub args: Vec<Expr>,
    /// Source location.
    pub span: Span,
}

/// A handle expression.
#[derive(Debug, Clone)]
pub struct HandleExpr {
    /// The expression being handled.
    pub body: Box<Expr>,
    /// Handler values specified with `with` clause.
    /// These are expressions that evaluate to Handler<A> values.
    pub handler_values: Vec<Expr>,
    /// Inline handlers for each ability method.
    pub handlers: Vec<Handler>,
    /// Optional else clause for the normal return value.
    pub else_clause: Option<Box<Expr>>,
}

/// A handler for an ability method.
#[derive(Debug, Clone)]
pub struct Handler {
    /// The ability being handled.
    pub ability: QualifiedName,
    /// The method being handled.
    pub method: Arc<str>,
    /// Parameter bindings for the ability arguments.
    pub params: Vec<Param>,
    /// The handler body.
    pub body: Expr,
    /// Source location.
    pub span: Span,
}

/// A handler literal expression: `{ method(params) => body, ... }`.
/// Creates a first-class handler value that can be stored, passed, and composed.
#[derive(Debug, Clone)]
pub struct HandlerLiteralExpr {
    /// The methods provided by this handler.
    pub methods: Vec<HandlerLiteralMethod>,
    /// Source span.
    pub span: Span,
}

/// A method in a handler literal.
#[derive(Debug, Clone)]
pub struct HandlerLiteralMethod {
    /// The method name.
    pub method: Arc<str>,
    /// Parameter bindings for the ability arguments.
    pub params: Vec<Param>,
    /// The handler body.
    pub body: Expr,
    /// Source location.
    pub span: Span,
}

/// A sandbox expression: `sandbox with Ability { body }`.
///
/// Restricts the abilities available within the body to only those specified.
/// This enables running untrusted code with limited capabilities.
#[derive(Debug, Clone)]
pub struct SandboxExpr {
    /// The abilities allowed within the sandbox.
    /// If empty, no abilities are allowed (pure computation only).
    pub allowed_abilities: Vec<QualifiedName>,
    /// The body expression to run in the sandboxed context.
    pub body: Box<Expr>,
    /// Source location.
    pub span: Span,
}

// ─────────────────────────────────────────────────────────────────────────────
// Statements
// ─────────────────────────────────────────────────────────────────────────────

/// A statement in a block.
#[derive(Debug, Clone)]
pub struct Stmt {
    /// The statement kind.
    pub kind: StmtKind,
    /// Source location.
    pub span: Span,
}

impl Stmt {
    /// Create a new statement.
    #[must_use]
    pub const fn new(kind: StmtKind, span: Span) -> Self {
        Self { kind, span }
    }
}

/// The kind of statement.
#[derive(Debug, Clone)]
pub enum StmtKind {
    /// Let binding: `let x = expr` or `let x: Type = expr`.
    Let(LetBinding),

    /// Expression statement: `expr;`.
    Expr(Expr),
}

/// A let binding.
#[derive(Debug, Clone)]
pub struct LetBinding {
    /// Unique binding ID.
    pub id: BindingId,
    /// Variable name.
    pub name: Arc<str>,
    /// Span of the variable name (for IDE features).
    pub name_span: Span,
    /// Optional type annotation.
    pub ty: Option<Type>,
    /// The initializer expression.
    pub init: Expr,
}

// ─────────────────────────────────────────────────────────────────────────────
// Top-level items
// ─────────────────────────────────────────────────────────────────────────────

/// A module containing top-level items.
#[derive(Debug, Clone, Default)]
pub struct Module {
    /// Module name (derived from file path).
    pub name: Arc<str>,
    /// Module-level documentation from `//!` comments.
    pub doc: Option<Arc<str>>,
    /// Items in the module.
    pub items: Vec<Item>,
}

/// A top-level item.
#[derive(Debug, Clone)]
pub struct Item {
    /// The item kind.
    pub kind: ItemKind,
    /// Source location.
    pub span: Span,
    /// Documentation from `///` comments.
    pub doc: Option<Arc<str>>,
}

impl Item {
    /// Create a new item.
    #[must_use]
    pub const fn new(kind: ItemKind, span: Span) -> Self {
        Self {
            kind,
            span,
            doc: None,
        }
    }

    /// Create a new item with documentation.
    #[must_use]
    pub fn with_doc(kind: ItemKind, span: Span, doc: Option<Arc<str>>) -> Self {
        Self { kind, span, doc }
    }
}

/// The kind of top-level item.
#[derive(Debug, Clone)]
pub enum ItemKind {
    /// A function definition.
    Function(FunctionDef),

    /// A constant definition.
    Const(ConstDef),

    /// A type alias.
    TypeAlias(TypeAliasDef),

    /// An enum definition.
    Enum(EnumDef),

    /// An ability definition.
    Ability(AbilityDef),

    /// A use/import statement.
    Use(UseDef),

    /// A trait definition.
    Trait(TraitDef),

    /// A trait implementation.
    Impl(ImplDef),
}

/// A function definition.
#[derive(Debug, Clone)]
pub struct FunctionDef {
    /// Function name.
    pub name: Arc<str>,
    /// Span of the function name (for go-to-definition).
    pub name_span: Span,
    /// Whether this function is public.
    pub is_public: bool,
    /// Type parameters (generics).
    pub type_params: Vec<TypeParam>,
    /// Parameters.
    pub params: Vec<Param>,
    /// Return type (required for public functions).
    pub ret_ty: Option<Type>,
    /// Abilities used by this function.
    pub abilities: Vec<QualifiedName>,
    /// The function body.
    pub body: Expr,
}

/// A type parameter (generic).
#[derive(Debug, Clone)]
pub struct TypeParam {
    /// Parameter name.
    pub name: Arc<str>,
    /// Source location.
    pub span: Span,
}

/// A constant definition.
#[derive(Debug, Clone)]
pub struct ConstDef {
    /// Constant name.
    pub name: Arc<str>,
    /// Span of the constant name (for go-to-definition).
    pub name_span: Span,
    /// Whether this constant is public.
    pub is_public: bool,
    /// Type annotation (required).
    pub ty: Type,
    /// The value expression.
    pub value: Expr,
}

/// A type alias definition.
///
/// If `unique_id` is `Some`, this is a nominal type that is incompatible
/// with structurally identical types (e.g., `unique(uuid) type UserId { value: string }`).
#[derive(Debug, Clone)]
pub struct TypeAliasDef {
    /// Type name.
    pub name: Arc<str>,
    /// Span of the type name (for go-to-definition).
    pub name_span: Span,
    /// Whether this type alias is public.
    pub is_public: bool,
    /// Type parameters (generics).
    pub type_params: Vec<TypeParam>,
    /// The aliased type (wrapped in `Type::Nominal` if `unique_id` is set).
    pub ty: Type,
    /// Optional UUID for nominal types. If set, makes this type incompatible
    /// with structurally identical types.
    pub unique_id: Option<Uuid>,
}

/// An enum definition.
///
/// Every enum is nominal: its `unique(<uuid>)` prefix is mandatory, so two
/// structurally identical enums are distinct types and an enum's methods get
/// uuid-based dispatch symbols. Lowering rejects a bare `enum` with no prefix.
#[derive(Debug, Clone)]
pub struct EnumDef {
    /// Enum name.
    pub name: Arc<str>,
    /// Span of the enum name (for go-to-definition).
    pub name_span: Span,
    /// Whether this enum is public.
    pub is_public: bool,
    /// Type parameters (generics).
    pub type_params: Vec<TypeParam>,
    /// Enum variants.
    pub variants: Vec<EnumVariant>,
    /// Nominal identity from the mandatory `unique(<uuid>)` prefix.
    pub uuid: Uuid,
}

/// An enum variant.
#[derive(Debug, Clone)]
pub struct EnumVariant {
    /// Variant name.
    pub name: Arc<str>,
    /// Optional payload type.
    pub payload: Option<Type>,
    /// Source location.
    pub span: Span,
}

/// An ability definition.
#[derive(Debug, Clone)]
pub struct AbilityDef {
    /// Ability name.
    pub name: Arc<str>,
    /// Span of the ability name (for go-to-definition).
    pub name_span: Span,
    /// Whether this ability is public.
    pub is_public: bool,
    /// Dependencies (other abilities this one requires).
    pub dependencies: Vec<QualifiedName>,
    /// Methods defined by this ability.
    pub methods: Vec<AbilityMethod>,
    /// Content-addressed identity, computed during type checking from the
    /// resolved method signatures. `None` until the module is checked; the
    /// compiler reads it rather than re-deriving the hash.
    pub resolved_id: Option<crate::types::AbilityId>,
}

/// An ability method signature.
#[derive(Debug, Clone)]
pub struct AbilityMethod {
    /// Method name.
    pub name: Arc<str>,
    /// Type parameters (generics).
    pub type_params: Vec<TypeParam>,
    /// Parameters.
    pub params: Vec<(Arc<str>, Type)>,
    /// Return type.
    pub ret_ty: Type,
    /// Source location.
    pub span: Span,
}

/// A use/import statement.
///
/// Examples:
/// - `use pkg::utils;` - Import module
/// - `use pkg::utils::helper;` - Import specific item
/// - `use pkg::utils::{helper, format};` - Import multiple items
/// - `use pkg::utils::*;` - Import all public items
/// - `use self::sibling;` - Relative import (same directory)
/// - `use super::parent;` - Parent directory import
/// - `use core::List;` - Standard library import
/// - `pub use pkg::other::Thing;` - Re-export
#[derive(Debug, Clone)]
pub struct UseDef {
    /// Whether this is a public re-export.
    pub is_public: bool,
    /// The import prefix determining the root.
    pub prefix: UsePrefix,
    /// Path segments after the prefix, with their source spans.
    pub path: Vec<(Arc<str>, Span)>,
    /// What to import from the path.
    pub kind: UseKind,
}

/// The prefix of a use path, determining the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsePrefix {
    /// `pkg::module` - Local package
    Pkg,
    /// `core::module` - Standard library
    Core,
    /// `platform::Ability` - Embedded platform ability module
    Platform,
    /// `self::sibling` - Same directory as current module
    Self_,
    /// `super::module` - Parent directory (can be chained: `super::super`)
    /// The number indicates how many levels up (1 for super, 2 for `super::super`)
    Super(usize),
}

/// What to import from a use path.
#[derive(Debug, Clone)]
pub enum UseKind {
    /// Import the module itself: `use pkg::utils;`
    /// Brings the module name into scope.
    Module,
    /// Import specific items: `use pkg::utils::{helper, format};`
    /// Each item carries the source span of its name (for IDE features:
    /// go-to-definition, references, and rename on a braced import name).
    Items(Vec<(Arc<str>, Span)>),
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait and Impl Definitions
// ─────────────────────────────────────────────────────────────────────────────

/// A trait definition.
///
/// Syntax: `trait Name<T> with Supertrait { fn method(self, ...): RetType; }`
#[derive(Debug, Clone)]
pub struct TraitDef {
    /// Trait name.
    pub name: Arc<str>,
    /// Span of the trait name.
    pub name_span: Span,
    /// Whether this trait is public.
    pub is_public: bool,
    /// Type parameters.
    pub type_params: Vec<TypeParam>,
    /// Supertraits that this trait requires.
    pub supertraits: Vec<QualifiedName>,
    /// Method signatures.
    pub methods: Vec<TraitMethod>,
}

/// A method signature in a trait definition.
#[derive(Debug, Clone)]
pub struct TraitMethod {
    /// Method name.
    pub name: Arc<str>,
    /// Span of the method name.
    pub name_span: Span,
    /// Type parameters for the method.
    pub type_params: Vec<TypeParam>,
    /// Whether this method takes self as first parameter.
    pub has_self: bool,
    /// Parameters (excluding self).
    pub params: Vec<(Arc<str>, Type)>,
    /// Return type.
    pub ret_ty: Type,
    /// Source span.
    pub span: Span,
}

/// A trait implementation or an inherent impl block.
///
/// Syntax: `impl<T> Trait for Type where T: Bound { fn method(self, ...) { body } }`
/// or, for inherent impls: `impl<T> Type { fn method(self, ...) { body } }`
#[derive(Debug, Clone)]
pub struct ImplDef {
    /// Type parameters for generic impls.
    pub type_params: Vec<TypeParam>,
    /// The trait being implemented; `None` for an inherent impl.
    pub trait_name: Option<QualifiedName>,
    /// The type implementing the trait.
    pub for_type: Type,
    /// Where clauses (type, trait bounds).
    pub where_clauses: Vec<WhereClause>,
    /// Method implementations.
    pub methods: Vec<ImplMethod>,
    /// Source span.
    pub span: Span,
}

/// A where clause for trait bounds.
#[derive(Debug, Clone)]
pub struct WhereClause {
    /// The type being constrained.
    pub ty: Type,
    /// Trait bounds.
    pub bounds: Vec<QualifiedName>,
}

/// A method implementation in an impl block.
#[derive(Debug, Clone)]
pub struct ImplMethod {
    /// Method name.
    pub name: Arc<str>,
    /// Span of the method name.
    pub name_span: Span,
    /// Type parameters for the method.
    pub type_params: Vec<TypeParam>,
    /// Whether this method takes `self` as its first parameter. Associated
    /// methods (e.g. `Default::default`) take no `self` and are called as
    /// `Type::method(...)` rather than through a receiver.
    pub has_self: bool,
    /// Binding ID for self parameter (unused when `has_self` is false).
    pub self_id: BindingId,
    /// Parameters (excluding self).
    pub params: Vec<Param>,
    /// Return type.
    pub ret_ty: Option<Type>,
    /// Declared abilities (`with Stdio, Log`). Enforced for inherent
    /// methods, which behave like public functions: no clause means pure.
    pub abilities: Vec<QualifiedName>,
    /// Method body.
    pub body: Expr,
    /// Source span.
    pub span: Span,
    /// Canonical function symbol for this method implementation
    /// (see `types::impl_method_symbol`). Filled in during type checking;
    /// the compiler registers the method under this name so it is
    /// content-addressed like any other function.
    pub resolved_symbol: Option<Arc<str>>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Builder helpers for testing
// ─────────────────────────────────────────────────────────────────────────────

impl Expr {
    /// Create a unit literal.
    #[must_use]
    pub fn unit() -> Self {
        Self::new(ExprKind::Unit, Span::default())
    }

    /// Create a boolean literal.
    #[must_use]
    pub fn bool(value: bool) -> Self {
        Self::new(ExprKind::Bool(value), Span::default())
    }

    /// Create a number literal.
    #[must_use]
    pub fn number(value: f64) -> Self {
        Self::new(ExprKind::Number(value), Span::default())
    }

    /// Create a string literal.
    #[must_use]
    pub fn string(value: impl Into<Arc<str>>) -> Self {
        Self::new(ExprKind::String(value.into()), Span::default())
    }

    /// Create a local variable reference.
    #[must_use]
    pub fn local(id: BindingId) -> Self {
        Self::new(ExprKind::Local(id), Span::default())
    }

    /// Create a named reference.
    #[must_use]
    pub fn name(name: impl Into<Arc<str>>) -> Self {
        Self::new(ExprKind::Name(QualifiedName::simple(name)), Span::default())
    }

    /// Create a binary operation.
    #[must_use]
    pub fn binary(op: BinaryOp, left: Expr, right: Expr) -> Self {
        Self::new(
            ExprKind::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                resolved_op: None,
            },
            Span::default(),
        )
    }

    /// Create a unary operation.
    #[must_use]
    pub fn unary(op: UnaryOp, operand: Expr) -> Self {
        Self::new(ExprKind::Unary(op, Box::new(operand)), Span::default())
    }

    /// Create an if expression.
    #[must_use]
    pub fn if_then_else(cond: Expr, then_branch: Expr, else_branch: Option<Expr>) -> Self {
        Self::new(
            ExprKind::If(
                Box::new(cond),
                Box::new(then_branch),
                else_branch.map(Box::new),
            ),
            Span::default(),
        )
    }

    /// Create a tuple expression.
    #[must_use]
    pub fn tuple(elements: Vec<Expr>) -> Self {
        Self::new(ExprKind::Tuple(elements), Span::default())
    }

    /// Create a record expression.
    #[must_use]
    pub fn record(fields: impl IntoIterator<Item = (impl Into<Arc<str>>, Expr)>) -> Self {
        let fields: Vec<_> = fields.into_iter().map(|(k, v)| (k.into(), v)).collect();
        Self::new(ExprKind::Record(fields), Span::default())
    }

    /// Create a function call.
    #[must_use]
    pub fn call(callee: Expr, args: Vec<Expr>) -> Self {
        Self::new(ExprKind::Call(Box::new(callee), args), Span::default())
    }

    /// Create a lambda expression.
    #[must_use]
    pub fn lambda(params: Vec<Param>, body: Expr) -> Self {
        Self::new(
            ExprKind::Lambda(Lambda {
                params,
                ret_ty: None,
                body: Box::new(body),
            }),
            Span::default(),
        )
    }

    /// Create a block expression.
    #[must_use]
    pub fn block(stmts: Vec<Stmt>, result: Option<Expr>) -> Self {
        Self::new(
            ExprKind::Block(stmts, result.map(Box::new)),
            Span::default(),
        )
    }

    /// Create a handler literal expression.
    #[must_use]
    pub fn handler_literal(methods: Vec<HandlerLiteralMethod>) -> Self {
        Self::new(
            ExprKind::HandlerLiteral(HandlerLiteralExpr {
                methods,
                span: Span::default(),
            }),
            Span::default(),
        )
    }

    /// Create a sandbox expression.
    #[must_use]
    pub fn sandbox(allowed_abilities: Vec<QualifiedName>, body: Expr) -> Self {
        Self::new(
            ExprKind::Sandbox(SandboxExpr {
                allowed_abilities,
                body: Box::new(body),
                span: Span::default(),
            }),
            Span::default(),
        )
    }

    /// Create a tuple index expression.
    #[must_use]
    pub fn tuple_index(tuple: Expr, index: u32) -> Self {
        Self::new(
            ExprKind::TupleIndex(Box::new(tuple), index),
            Span::default(),
        )
    }

    /// Create a field access expression.
    #[must_use]
    pub fn field_access(record: Expr, field: impl Into<Arc<str>>) -> Self {
        Self::new(
            ExprKind::RecordField(Box::new(record), field.into()),
            Span::default(),
        )
    }

    /// Create a match expression.
    #[must_use]
    pub fn match_expr(scrutinee: Expr, arms: Vec<MatchArm>) -> Self {
        Self::new(ExprKind::Match(Box::new(scrutinee), arms), Span::default())
    }

    /// Create a variable reference (named reference).
    #[must_use]
    pub fn variable(name: impl Into<Arc<str>>) -> Self {
        Self::name(name)
    }
}

impl HandlerLiteralMethod {
    /// Create a new handler literal method.
    #[must_use]
    pub fn new(method: impl Into<Arc<str>>, params: Vec<Param>, body: Expr) -> Self {
        Self {
            method: method.into(),
            params,
            body,
            span: Span::default(),
        }
    }
}

impl Param {
    /// Create a new parameter.
    #[must_use]
    pub fn new(id: BindingId, name: impl Into<Arc<str>>) -> Self {
        Self {
            id,
            name: name.into(),
            ty: None,
            span: Span::default(),
        }
    }

    /// Create a parameter with a type annotation.
    #[must_use]
    pub fn with_type(id: BindingId, name: impl Into<Arc<str>>, ty: Type) -> Self {
        Self {
            id,
            name: name.into(),
            ty: Some(ty),
            span: Span::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_span_merge() {
        let s1 = Span::new(10, 20);
        let s2 = Span::new(15, 30);
        let merged = s1.merge(s2);
        assert_eq!(merged, Span::new(10, 30));
    }

    #[test]
    fn test_expr_builders() {
        let expr = Expr::binary(BinaryOp::Add, Expr::number(1.0), Expr::number(2.0));

        if let ExprKind::Binary {
            op, left, right, ..
        } = expr.kind
        {
            assert_eq!(op, BinaryOp::Add);
            assert!(matches!(left.kind, ExprKind::Number(n) if n == 1.0));
            assert!(matches!(right.kind, ExprKind::Number(n) if n == 2.0));
        } else {
            panic!("Expected binary expression");
        }
    }

    #[test]
    fn test_qualified_name() {
        let simple = QualifiedName::simple("foo");
        assert!(simple.path.is_empty());
        assert_eq!(&*simple.name, "foo");

        let qualified = QualifiedName::qualified(vec!["std", "io"], "print");
        assert_eq!(qualified.path.len(), 2);
        assert_eq!(&*qualified.path[0], "std");
        assert_eq!(&*qualified.path[1], "io");
        assert_eq!(&*qualified.name, "print");
    }

    #[test]
    fn test_lambda_expr() {
        let lambda = Expr::lambda(
            vec![Param::new(0, "x"), Param::new(1, "y")],
            Expr::binary(BinaryOp::Add, Expr::local(0), Expr::local(1)),
        );

        if let ExprKind::Lambda(l) = lambda.kind {
            assert_eq!(l.params.len(), 2);
            assert_eq!(&*l.params[0].name, "x");
            assert_eq!(&*l.params[1].name, "y");
        } else {
            panic!("Expected lambda expression");
        }
    }

    #[test]
    fn test_record_expr() {
        let record = Expr::record([("x", Expr::number(1.0)), ("y", Expr::number(2.0))]);

        if let ExprKind::Record(fields) = record.kind {
            assert_eq!(fields.len(), 2);
            assert_eq!(&*fields[0].0, "x");
            assert_eq!(&*fields[1].0, "y");
        } else {
            panic!("Expected record expression");
        }
    }

    #[test]
    fn test_binary_op_classification() {
        assert!(BinaryOp::Add.is_arithmetic());
        assert!(BinaryOp::Sub.is_arithmetic());
        assert!(!BinaryOp::Add.is_comparison());
        assert!(!BinaryOp::Add.is_logical());

        assert!(BinaryOp::Eq.is_comparison());
        assert!(BinaryOp::Lt.is_comparison());
        assert!(!BinaryOp::Eq.is_arithmetic());

        assert!(BinaryOp::And.is_logical());
        assert!(BinaryOp::Or.is_logical());
        assert!(!BinaryOp::And.is_arithmetic());
    }
}

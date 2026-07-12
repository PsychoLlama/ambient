//! CST nodes for trait definitions and impl blocks.

use std::sync::Arc;

use ambient_engine::ast::Span;

use super::{CstExpr, CstIdent, CstQualifiedName, CstTypeExpr, CstTypeParam};

// ─────────────────────────────────────────────────────────────────────────────
// Trait Definition
// ─────────────────────────────────────────────────────────────────────────────

/// A trait definition.
///
/// Syntax: `unique(<uuid>) trait Name<T> { fn method(self, ...): RetType; }`
#[derive(Debug, Clone)]
pub struct CstTraitDef {
    /// Whether public (`pub unique(...) trait`).
    pub is_public: bool,
    /// The `unique(<uuid>)` identity. Mandatory — traits are nominal like
    /// enums and abilities; lowering rejects a `None`.
    pub unique_id: Option<Arc<str>>,
    /// Trait name.
    pub name: CstIdent,
    /// Type parameters.
    pub type_params: Vec<CstTypeParam>,
    /// Supertraits (`with Trait1, Trait2`).
    pub supertraits: Vec<CstQualifiedName>,
    /// Method signatures.
    pub methods: Vec<CstTraitMethod>,
    /// Source span.
    pub span: Span,
}

/// A method signature in a trait definition.
#[derive(Debug, Clone)]
pub struct CstTraitMethod {
    /// Method name.
    pub name: CstIdent,
    /// Type parameters for the method.
    pub type_params: Vec<CstTypeParam>,
    /// Parameters (first may be `self`).
    pub params: Vec<CstTraitParam>,
    /// Return type.
    pub ret_ty: CstTypeExpr,
    /// Declared abilities (`with E` or `with Stdio, Log`). A trait method's
    /// effect row is part of its dispatch contract, so it is declared here
    /// exactly like a free function's `with` clause.
    pub abilities: Vec<CstQualifiedName>,
    /// Source span.
    pub span: Span,
}

/// A parameter in a trait method (supports `self`).
#[derive(Debug, Clone)]
pub struct CstTraitParam {
    /// Parameter kind.
    pub kind: CstTraitParamKind,
    /// Source span.
    pub span: Span,
}

/// The kind of trait method parameter.
#[derive(Debug, Clone)]
pub enum CstTraitParamKind {
    /// Bare `self` parameter (type is implicit Self).
    SelfParam,
    /// Named parameter with type: `name: Type`.
    Named { name: CstIdent, ty: CstTypeExpr },
}

// ─────────────────────────────────────────────────────────────────────────────
// Impl Definition
// ─────────────────────────────────────────────────────────────────────────────

/// A trait implementation or an inherent impl block.
///
/// Syntax: `impl<T> Trait for Type where T: Bound { fn method(self, ...) { body } }`
/// or, for inherent impls: `impl<T> Type { fn method(self, ...) { body } }`
#[derive(Debug, Clone)]
pub struct CstImplDef {
    /// Type parameters for generic impls.
    pub type_params: Vec<CstTypeParam>,
    /// The trait being implemented; `None` for an inherent impl.
    pub trait_name: Option<CstQualifiedName>,
    /// The type implementing the trait.
    pub for_type: CstTypeExpr,
    /// Where clauses.
    pub where_clauses: Vec<CstWhereClause>,
    /// Method implementations.
    pub methods: Vec<CstImplMethod>,
    /// Source span.
    pub span: Span,
}

/// A method implementation in an impl block.
#[derive(Debug, Clone)]
pub struct CstImplMethod {
    /// Method name.
    pub name: CstIdent,
    /// Type parameters for the method.
    pub type_params: Vec<CstTypeParam>,
    /// Parameters (first may be `self`).
    pub params: Vec<CstTraitParam>,
    /// Return type.
    pub ret_ty: Option<CstTypeExpr>,
    /// Declared abilities (`with Stdio, Log`).
    pub abilities: Vec<CstQualifiedName>,
    /// Method body.
    pub body: CstExpr,
    /// Source span.
    pub span: Span,
}

/// A where clause for trait bounds.
///
/// Syntax: `T: Trait1 + Trait2`
#[derive(Debug, Clone)]
pub struct CstWhereClause {
    /// The type being constrained.
    pub ty: CstTypeExpr,
    /// Trait bounds.
    pub bounds: Vec<CstQualifiedName>,
    /// Source span.
    pub span: Span,
}

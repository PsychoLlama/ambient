//! Top-level item definitions: modules, items, and the declaration forms
//! (`fn`, `extern fn`, `const`, `struct`, `enum`, `ability`, `trait`,
//! `impl`, `use`). Split from the expression AST; re-exported through
//! [`crate::ast`] so paths are unchanged.

use std::sync::Arc;

use uuid::Uuid;

use super::{BindingId, Expr, Param, QualifiedName, Span};
use crate::types::Type;

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

    /// A struct (record) definition.
    Struct(StructDef),

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

    /// An `extern fn` declaration: a body-less signature whose
    /// implementation is provided by the host at compile time.
    ExternFn(ExternFnDef),
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

/// An `extern fn` declaration.
///
/// An extern function is a signature without a body: the implementation is a
/// native function provided by the host (the engine for `core::`, an embedder
/// for its own modules) and attached at compile time through a
/// [`crate::natives::NativeRegistry`]. The declaration carries no identity of
/// its own — the registry supplies a stable UUID, which is the function's
/// *content* identity (the name is only the compile-time lookup key, so
/// renaming an extern fn never changes a hash).
///
/// Extern fns are pure by construction: they take no `with` clause, and the
/// full signature (typed params, return type) is mandatory because there is
/// no body to infer from. Otherwise they are ordinary items — `pub` gates
/// export, they import through `use`, and they are first-class values.
#[derive(Debug, Clone)]
pub struct ExternFnDef {
    /// Function name.
    pub name: Arc<str>,
    /// Span of the function name (for go-to-definition).
    pub name_span: Span,
    /// Whether this function is public.
    pub is_public: bool,
    /// Type parameters (generics).
    pub type_params: Vec<TypeParam>,
    /// Parameters (every parameter carries a declared type).
    pub params: Vec<Param>,
    /// Return type (mandatory).
    pub ret_ty: Type,
}

/// A type parameter (generic).
#[derive(Debug, Clone)]
pub struct TypeParam {
    /// Parameter name.
    pub name: Arc<str>,
    /// Whether this is an **ability (row) variable** (`E!` in the source),
    /// declaring a polymorphic effect row rather than a type. An ability
    /// variable names an [`crate::types::AbilitySet`] tail in ability
    /// positions (the item's `with` clause and function-type `with` lists);
    /// it is never a type and carries no trait bounds (so it contributes
    /// nothing to [`dict_params`]).
    pub is_ability: bool,
    /// Trait bounds (`T: Eq + Ord`), as written. The checker resolves each
    /// to a trait identity; an impl's `where` clauses fold into these at
    /// lowering, so bounds have exactly one AST representation.
    pub bounds: Vec<QualifiedName>,
    /// Source location.
    pub span: Span,
}

impl TypeParam {
    /// An unbounded type parameter.
    #[must_use]
    pub fn plain(name: impl Into<Arc<str>>, span: Span) -> Self {
        Self {
            name: name.into(),
            is_ability: false,
            bounds: Vec::new(),
            span,
        }
    }
}

/// The dictionary-parameter list an item's type parameters imply: one
/// `(param name, bound reference)` per distinct bound, in declaration order.
///
/// This is the **single authority** on dictionary order and count. The
/// checker resolves each bound reference to a trait identity and solves
/// call-site constraints against the list; the compiler allocates one hidden
/// trailing parameter per entry. Both derive from this function — and both
/// read the same [`QualifiedName::resolved`] the resolve pass wrote — so they
/// can never disagree.
///
/// Dedup is by trait **identity**: two bounds on one parameter collapse iff
/// they name the same trait. When the resolve pass ran, that is the resolved
/// `Fqn` (so a bare `Show` and a qualified `m::Show` collapse, and two
/// same-named traits from different modules do *not*); registry-less (no
/// resolution), it falls back to the spelled path+name, the purely syntactic
/// rule the resolver-less compiler applies identically.
#[must_use]
pub fn dict_params(type_params: &[TypeParam]) -> Vec<(Arc<str>, &QualifiedName)> {
    let mut out: Vec<(Arc<str>, &QualifiedName)> = Vec::new();
    for tp in type_params {
        for bound in &tp.bounds {
            let duplicate = out
                .iter()
                .any(|(param, existing)| *param == tp.name && existing.same_target(bound));
            if !duplicate {
                out.push((Arc::clone(&tp.name), bound));
            }
        }
    }
    out
}

/// A constant definition.
#[derive(Debug, Clone)]
pub struct ConstDef {
    /// Unique binding ID. Distinguishes this declaration from every other
    /// binding in the module (used when a block-scoped `const` enters the
    /// checker's local environment, mirroring [`LetBinding::id`]).
    pub id: BindingId,
    /// Constant name.
    pub name: Arc<str>,
    /// Span of the constant name (for go-to-definition).
    pub name_span: Span,
    /// Whether this constant is public.
    pub is_public: bool,
    /// Type annotation, or `None` when omitted — inferred from the literal
    /// initializer (a `const` value is always a primitive literal, so its
    /// type is determined by the value).
    pub ty: Option<Type>,
    /// The value expression.
    pub value: Expr,
}

/// A struct (record) definition: `struct Foo { fields }`.
///
/// The body (`ty`) is always a record type. If `unique_id` is `Some`, this is a
/// nominal type that is incompatible with structurally identical types (e.g.,
/// `unique(uuid) struct UserId { value: string }`), and `ty` is wrapped in
/// `Type::Nominal`.
#[derive(Debug, Clone)]
pub struct StructDef {
    /// Struct name.
    pub name: Arc<str>,
    /// Span of the struct name (for go-to-definition).
    pub name_span: Span,
    /// Whether this struct is public.
    pub is_public: bool,
    /// Type parameters (generics).
    pub type_params: Vec<TypeParam>,
    /// The record body (wrapped in `Type::Nominal` if `unique_id` is set).
    pub ty: Type,
    /// Optional UUID for a nominal identity. If set, makes this type
    /// incompatible with structurally identical types.
    pub unique_id: Option<Uuid>,
    /// Whether this struct is `extern`: an engine-provided type. User code may
    /// name it in type positions and read its fields, but may not construct it
    /// (neither `T { .. }` nor a bare unit-form `T`). `extern` requires
    /// `unique(...)`, so `unique_id` is always `Some` when this is set.
    pub is_extern: bool,
}

impl StructDef {
    /// Whether this is a *unit* struct by shape: a `unique` struct with an empty
    /// body (`unique(<uuid>) struct Origin;`). Post-parser such a type is
    /// exactly a `Type::Nominal` wrapping a zero-field `Type::Record`. This is
    /// the pure *shape* predicate; use [`Self::is_unit_value`] to decide whether
    /// the bare name denotes a value (an `extern` unit struct is a type but not
    /// a constructable value).
    #[must_use]
    pub fn is_unit(&self) -> bool {
        matches!(
            &self.ty,
            Type::Nominal(n)
                if matches!(&*n.inner, Type::Record(r) if r.fields.is_empty())
        )
    }

    /// Whether this unit struct's bare name denotes a *value* — a single value
    /// constructed by its bare name, the same dual type/value identity a nullary
    /// enum variant carries. This is the single source of truth every site keys
    /// off to bind, resolve, and compile that value. An `extern` unit struct has
    /// the unit *shape* but is engine-provided, so it is a type only, never a
    /// constructable value.
    #[must_use]
    pub fn is_unit_value(&self) -> bool {
        self.is_unit() && !self.is_extern
    }
}

/// A type alias definition: `type Foo = Bar`.
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
    /// The aliased type.
    pub ty: Type,
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
///
/// Abilities are nominal, like enums: the `unique(<uuid>)` prefix is the
/// ability's identity, so renaming or moving the declaration never changes
/// it, and two same-shaped abilities in different modules stay distinct.
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
    /// Nominal identity from the mandatory `unique(<uuid>)` prefix.
    pub uuid: Uuid,
    /// The uuid-derived identity, written back during type checking; the
    /// compiler reads it rather than re-deriving.
    pub resolved_id: Option<crate::types::AbilityId>,
}

/// An ability method: a signature plus its default implementation — the
/// body that runs when a perform reaches no handler. The body compiles to
/// an ordinary content-addressed function, and its hash is folded into the
/// method's identity (see `ambient_core::MethodKey`), so two same-signature
/// methods with different behavior are distinct methods.
#[derive(Debug, Clone)]
pub struct AbilityMethod {
    /// Method name.
    pub name: Arc<str>,
    /// Type parameters (generics).
    pub type_params: Vec<TypeParam>,
    /// Parameters. Every parameter carries a declared type (enforced in
    /// lowering); the binding IDs bind the default body's scope.
    pub params: Vec<Param>,
    /// Return type.
    pub ret_ty: Type,
    /// Default implementation. `None` only for the `Exception` carve-out
    /// (its unhandled behavior is the VM's uncaught-exception path); the
    /// checker rejects a missing body everywhere else.
    pub body: Option<Expr>,
    /// Hash of the canonical signature rendering, written back during type
    /// checking (like `AbilityDef::resolved_id`); the compiler reads it to
    /// derive the method's `MethodKey` rather than re-rendering types.
    pub resolved_signature: Option<ambient_core::SignatureHash>,
    /// Source location.
    pub span: Span,
}

/// One flattened use/import declaration.
///
/// Source `use` items are Rust-style use-trees — nested brace groups,
/// `as` aliases, keyword or alias roots — and lowering flattens each
/// tree into one `UseDef` per imported leaf. Braces are pure grouping:
/// `use a::b::{c, d};` is exactly `use a::b::c; use a::b::d;`. Each leaf
/// names an entity by its final segment, and the resolver binds every
/// namespace meaning that exists (submodule, value, type, ability).
///
/// Examples (each line is one `UseDef` after flattening):
/// - `use pkg::utils;` — whole-module import
/// - `use pkg::utils::helper;` — item import
/// - `use core::primitives::number::sqrt as root2;` — aliased import
/// - `use utils::inner;` — `Local` root: `utils` is a module alias from
///   an earlier `use`
/// - `pub use pkg::other::Thing;` — re-export
#[derive(Debug, Clone)]
pub struct UseDef {
    /// Whether this is a public re-export.
    pub is_public: bool,
    /// The import prefix determining the root.
    pub prefix: UsePrefix,
    /// Path segments after the prefix, with their source spans. For a
    /// `Local` prefix the first segment is the in-scope module alias the
    /// path is rooted at.
    pub path: Vec<(Arc<str>, Span)>,
    /// The local binding name when renamed with `as`.
    pub alias: Option<(Arc<str>, Span)>,
}

impl UseDef {
    /// The local name this import binds: the alias if renamed, else the
    /// final path segment.
    #[must_use]
    pub fn local_name(&self) -> Option<&Arc<str>> {
        self.alias
            .as_ref()
            .map(|(name, _)| name)
            .or_else(|| self.path.last().map(|(name, _)| name))
    }
}

/// The prefix of a use path, determining the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsePrefix {
    /// `pkg::module` - Local package
    Pkg,
    /// `core::module` - Standard library
    Core,
    /// `self::sibling` - Same directory as current module
    Self_,
    /// `super::module` - Parent directory (can be chained: `super::super`)
    /// The number indicates how many levels up (1 for super, 2 for `super::super`)
    Super(usize),
    /// `alias::module` - Rooted at a module alias bound by another `use`
    /// in the same scope. The alias is the path's first segment.
    Local,
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait and Impl Definitions
// ─────────────────────────────────────────────────────────────────────────────

/// A trait definition.
///
/// Syntax: `unique(<uuid>) trait Name<T> with Supertrait { fn method(self, ...): RetType; }`
#[derive(Debug, Clone)]
pub struct TraitDef {
    /// Trait name.
    pub name: Arc<str>,
    /// Span of the trait name.
    pub name_span: Span,
    /// Whether this trait is public.
    pub is_public: bool,
    /// The trait's nominal identity, from the mandatory `unique(<uuid>)`
    /// prefix. Bounds, impl coherence, and dispatch symbols key off this —
    /// never the name — so renames and moves never change what a bound
    /// means, and same-shaped traits in different modules never unify.
    pub uuid: Uuid,
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
    /// Declared abilities (the method's `with` clause). `E` here names a
    /// method-level ability (row) variable from `type_params`; a concrete
    /// name resolves to its ability id.
    pub abilities: Vec<QualifiedName>,
    /// Source span.
    pub span: Span,
}

/// A trait implementation or an inherent impl block.
///
/// Syntax: `impl<T: Bound> Trait for Type { fn method(self, ...) { body } }`
/// or, for inherent impls: `impl<T> Type { fn method(self, ...) { body } }`.
/// A trailing `where T: Bound` clause is surface syntax only — lowering
/// folds it into the matching parameter's [`TypeParam::bounds`].
#[derive(Debug, Clone)]
pub struct ImplDef {
    /// Type parameters for generic impls (with their trait bounds).
    pub type_params: Vec<TypeParam>,
    /// The trait being implemented; `None` for an inherent impl.
    pub trait_name: Option<QualifiedName>,
    /// The type implementing the trait.
    pub for_type: Type,
    /// Method implementations.
    pub methods: Vec<ImplMethod>,
    /// Source span.
    pub span: Span,
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

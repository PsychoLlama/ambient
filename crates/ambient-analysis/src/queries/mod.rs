//! Position-based queries over the typed AST.
//!
//! These serve IDE features (hover, go-to-definition) and the REPL. They
//! are pure AST walks — resolution against other modules goes through the
//! engine's `ModuleRegistry`, never a parallel index.

use std::sync::Arc;

use ambient_engine::ast::{
    Expr, ExprKind, Item, ItemKind, Module, Pattern, PatternKind, QualifiedName, Span, StmtKind,
};
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::{ExportKind, ModuleRegistry, ResolvedImport};
use ambient_engine::types::Type;

mod bindings;
mod members;
mod signature;

use bindings::find_binding_definition;
pub use members::{
    MissingImplMembers, ReceiverMember, missing_impl_members_at_offset, receiver_members,
    receiver_type_at,
};
pub use signature::{
    assoc_type_signature, expr_signature, extern_fn_signature, item_doc, item_signature,
    method_signature_at, module_doc, module_signature, type_params_signature,
};

/// Find the expression at a given byte offset in the module.
#[must_use]
pub fn find_expr_at_offset(module: &Module, offset: u32) -> Option<&Expr> {
    for item in &module.items {
        if let Some(expr) = find_expr_in_item_at_offset(&item.kind, offset) {
            return Some(expr);
        }
    }
    None
}

/// Find the item definition at a given byte offset (on its name).
#[must_use]
pub fn find_item_at_offset(module: &Module, offset: u32) -> Option<&Item> {
    module.items.iter().find(|item| {
        item_name_span(item).is_some_and(|span| offset >= span.start && offset <= span.end)
    })
}

/// The span of an item's defining name, if it has one.
#[must_use]
pub fn item_name_span(item: &Item) -> Option<Span> {
    match &item.kind {
        ItemKind::Function(f) => Some(f.name_span),
        ItemKind::Const(c) => Some(c.name_span),
        ItemKind::Struct(s) => Some(s.name_span),
        ItemKind::TypeAlias(t) => Some(t.name_span),
        ItemKind::Enum(e) => Some(e.name_span),
        ItemKind::Ability(a) => Some(a.name_span),
        ItemKind::Trait(t) => Some(t.name_span),
        ItemKind::ExternFn(e) => Some(e.name_span),
        ItemKind::Set(s) => Some(s.name_span),
        ItemKind::Use(_) | ItemKind::Impl(_) => None,
    }
}

/// The name of an item, if it defines one.
#[must_use]
pub fn item_name(item: &Item) -> Option<&std::sync::Arc<str>> {
    match &item.kind {
        ItemKind::Function(f) => Some(&f.name),
        ItemKind::Const(c) => Some(&c.name),
        ItemKind::Struct(s) => Some(&s.name),
        ItemKind::TypeAlias(t) => Some(&t.name),
        ItemKind::Enum(e) => Some(&e.name),
        ItemKind::Ability(a) => Some(&a.name),
        ItemKind::Trait(t) => Some(&t.name),
        ItemKind::ExternFn(e) => Some(&e.name),
        ItemKind::Set(s) => Some(&s.name),
        ItemKind::Use(_) | ItemKind::Impl(_) => None,
    }
}

/// An associated-type declaration or binding whose *name* contains an offset,
/// with the declaring item for context — enough for a frontend to render
/// `type X;` / `type X = T;` hovers without re-walking the AST.
#[derive(Debug)]
pub enum AssocTypeAt<'a> {
    /// `type X;` declared in a trait body.
    TraitDecl {
        trait_def: &'a ambient_engine::ast::TraitDef,
        decl: &'a ambient_engine::ast::TraitAssocType,
    },
    /// `type X = T;` bound in an impl body.
    ImplBinding {
        impl_def: &'a ambient_engine::ast::ImplDef,
        binding: &'a ambient_engine::ast::ImplAssocType,
    },
}

/// Find the associated-type declaration or binding whose name span contains
/// `offset`, if any.
#[must_use]
pub fn find_assoc_type_at_offset(module: &Module, offset: u32) -> Option<AssocTypeAt<'_>> {
    let contains = |span: &Span| offset >= span.start && offset <= span.end;
    module.items.iter().find_map(|item| match &item.kind {
        ItemKind::Trait(trait_def) => trait_def
            .assoc_types
            .iter()
            .find(|a| contains(&a.name_span))
            .map(|decl| AssocTypeAt::TraitDecl { trait_def, decl }),
        ItemKind::Impl(impl_def) => impl_def
            .assoc_types
            .iter()
            .find(|a| contains(&a.name_span))
            .map(|binding| AssocTypeAt::ImplBinding { impl_def, binding }),
        _ => None,
    })
}

fn find_expr_in_item_at_offset(item: &ItemKind, offset: u32) -> Option<&Expr> {
    match item {
        ItemKind::Function(f) => find_expr_in_tree(&f.body, offset),
        ItemKind::Const(c) => find_expr_in_tree(&c.value, offset),
        ItemKind::Impl(i) => i
            .methods
            .iter()
            .find_map(|m| find_expr_in_tree(&m.body, offset)),
        _ => None,
    }
}

fn find_expr_in_tree(expr: &Expr, offset: u32) -> Option<&Expr> {
    // Check if offset is within this expression's span.
    if offset < expr.span.start || offset > expr.span.end {
        return None;
    }

    match &expr.kind {
        // Leaf nodes - these are directly hoverable
        ExprKind::Unit
        | ExprKind::Bool(_)
        | ExprKind::Number(_)
        | ExprKind::String(_)
        | ExprKind::Local(_)
        | ExprKind::Name(_)
        | ExprKind::Perform(_)
        | ExprKind::Resume(_)
        | ExprKind::HandlerLiteral(_) => Some(expr),

        // Container expressions - only return child, never the container itself
        ExprKind::Block(stmts, result) => {
            for stmt in stmts {
                // Check if offset is within this statement's span
                if offset >= stmt.span.start && offset <= stmt.span.end {
                    match &stmt.kind {
                        StmtKind::Let(binding) => {
                            // First try to find within the init expression
                            if let Some(e) = find_expr_in_tree(&binding.init, offset) {
                                return Some(e);
                            }
                            // If cursor is on the binding name or elsewhere in the let,
                            // return the init expression so hover shows the variable's type
                            return Some(&binding.init);
                        }
                        StmtKind::Expr(e) => {
                            return find_expr_in_tree(e, offset);
                        }
                        StmtKind::Use(_) => return None,
                        StmtKind::Const(const_def) => {
                            // Hover on the value, or on the name (which shows
                            // the const's value/type), returns the value expr.
                            if let Some(e) = find_expr_in_tree(&const_def.value, offset) {
                                return Some(e);
                            }
                            return Some(&const_def.value);
                        }
                    }
                }
            }
            result.as_ref().and_then(|e| find_expr_in_tree(e, offset))
        }
        ExprKind::Lambda(lambda) => find_expr_in_tree(&lambda.body, offset),
        ExprKind::Handle(handle) => find_expr_in_tree(&handle.body, offset)
            .or_else(|| {
                handle.handlers.iter().find_map(|h| {
                    // Descend into a handler literal's arm bodies (the literal
                    // node itself is a hover leaf); other handler expressions
                    // recurse normally.
                    if let ExprKind::HandlerLiteral(lit) = &h.kind {
                        lit.methods
                            .iter()
                            .find_map(|m| find_expr_in_tree(&m.body, offset))
                    } else {
                        find_expr_in_tree(h, offset)
                    }
                })
            })
            .or_else(|| {
                handle
                    .else_clause
                    .as_ref()
                    .and_then(|e| find_expr_in_tree(e, offset))
            }),
        ExprKind::Sandbox(sandbox) => find_expr_in_tree(&sandbox.body, offset),
        ExprKind::Match(scrutinee, arms) => find_expr_in_tree(scrutinee, offset).or_else(|| {
            arms.iter()
                .find_map(|arm| find_expr_in_tree(&arm.body, offset))
        }),

        // Expressions with children - search children, fall back to self
        ExprKind::Binary { left, right, .. } => find_expr_in_tree(left, offset)
            .or_else(|| find_expr_in_tree(right, offset))
            .or(Some(expr)),
        ExprKind::Unary(_, operand) => find_expr_in_tree(operand, offset).or(Some(expr)),
        ExprKind::Call(callee, args) => find_expr_in_tree(callee, offset)
            .or_else(|| args.iter().find_map(|a| find_expr_in_tree(a, offset)))
            .or(Some(expr)),
        ExprKind::If(cond, then_br, else_br) => find_expr_in_tree(cond, offset)
            .or_else(|| find_expr_in_tree(then_br, offset))
            .or_else(|| else_br.as_ref().and_then(|e| find_expr_in_tree(e, offset)))
            .or(Some(expr)),
        ExprKind::Tuple(elements) | ExprKind::List(elements) => elements
            .iter()
            .find_map(|e| find_expr_in_tree(e, offset))
            .or(Some(expr)),
        ExprKind::Record(fields) => fields
            .iter()
            .find_map(|(_, e)| find_expr_in_tree(e, offset))
            .or(Some(expr)),
        ExprKind::TypedRecord { fields, .. } => fields
            .iter()
            .find_map(|(_, e)| find_expr_in_tree(e, offset))
            .or(Some(expr)),
        ExprKind::RecordField(object, _) => find_expr_in_tree(object, offset).or(Some(expr)),
        ExprKind::TupleIndex(tuple, _) => find_expr_in_tree(tuple, offset).or(Some(expr)),
        ExprKind::MethodCall { receiver, args, .. } => args
            .iter()
            .find_map(|e| find_expr_in_tree(e, offset))
            .or_else(|| find_expr_in_tree(receiver, offset))
            .or(Some(expr)),
    }
}

/// Format a type for display.
///
/// Delegates to the engine's canonical `Display` implementation so hover,
/// completions, compiler errors, and the REPL all render types the same
/// way.
#[must_use]
pub fn format_type(ty: &Type) -> String {
    // Close unconstrained effect rows: a generalized-but-uninstantiated `E!`
    // means "no known effects", so it must not render as `with E23!` in a
    // signature. Display-only — never applied to diagnostics or hashing.
    ty.close_rows_for_display().to_string()
}

/// Format a type for hover, surfacing the module-qualified identity of a
/// primitive (`core::primitives::string`) that its bare `Display` name (`String`) omits.
///
/// Hover is the one surface that shows a nominal type's fully-qualified
/// identity; diagnostics, completions, and the REPL keep the terse bare
/// `Display` via [`format_type`]. The FQN comes from engine data (the
/// reserved-uuid → `core::*` table on `Primitive`), not an LSP-private
/// resolver, so the renderer stays a pure view over engine facts.
#[must_use]
pub fn format_type_hover(ty: &Type) -> String {
    match ty.as_primitive() {
        Some(prim) => prim.fqn().to_string(),
        // Close unconstrained effect rows so a function-typed hover (a local
        // bound to a lambda) doesn't leak `with E23!`. Display-only.
        None => ty.close_rows_for_display().to_string(),
    }
}

/// Where a definition lives.
#[derive(Debug, Clone)]
pub struct Definition {
    /// The span of the definition in its module's source.
    pub span: Span,
    /// The module that defines it; `None` means the current module.
    pub module: Option<ModulePath>,
}

impl Definition {
    fn local(span: Span) -> Self {
        Self { span, module: None }
    }
}

/// Find the definition for whatever is at `offset`, resolving across
/// modules through the registry — the same machinery the type checker
/// uses for imports, so navigation and checking can't disagree.
#[must_use]
pub fn find_definition(
    module: &Module,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    offset: u32,
) -> Option<Definition> {
    if let Some(expr) = find_expr_at_offset(module, offset) {
        match &expr.kind {
            ExprKind::Local(binding_id) => {
                for item in &module.items {
                    if let ItemKind::Function(func) = &item.kind
                        && let Some(def_span) = find_binding_definition(func, *binding_id)
                    {
                        return Some(Definition::local(def_span));
                    }
                }
                return None;
            }
            ExprKind::Name(qname) => {
                return resolve_qualified_name(module, module_path, registry, qname);
            }
            ExprKind::Call(callee, _) => {
                if let ExprKind::Name(qname) = &callee.kind {
                    return resolve_qualified_name(module, module_path, registry, qname);
                }
                return None;
            }
            _ => return None,
        }
    }

    // No expression under the cursor: it may be an enum-variant *pattern*
    // (`Some(n)` in a match arm). Patterns aren't expressions, so the walk
    // above never reaches them; resolve the variant name through the same
    // registry path a constructor uses.
    let qname = find_variant_pattern_at_offset(module, offset)?;
    resolve_qualified_name(module, module_path, registry, qname)
}

/// Resolve the name/callee at `offset` to the declaring [`Item`], for hover.
///
/// A callsite name (`title_case(x)`) or a bare name reference is an
/// `ExprKind::Name`; on its own it only carries the checker-*inferred*
/// (instantiated) type, so hover would show a bare type with no parameter
/// names or doc. This runs the same registry-backed resolution as
/// go-to-definition ([`resolve_qualified_name`]) and maps the resulting
/// [`Definition`] back to the `&Item` in its defining module's AST, so the
/// caller can render a full signature (params + doc) via `format_item_hover` —
/// for package items and builtins alike (the registry holds their ASTs).
///
/// Locals are deliberately excluded: a local keeps type-shaped hover (there is
/// no item to render). Returns `None` when the cursor isn't on a name/callee,
/// or the name doesn't resolve to an item this walk can locate (e.g. an enum
/// variant, whose `Definition` points at the variant, not an item name span).
#[must_use]
pub fn definition_item<'a>(
    module: &'a Module,
    module_path: &ModulePath,
    registry: &'a ModuleRegistry,
    offset: u32,
) -> Option<&'a Item> {
    let expr = find_expr_at_offset(module, offset)?;
    let qname = match &expr.kind {
        ExprKind::Name(qname) => qname,
        ExprKind::Call(callee, _) => match &callee.kind {
            ExprKind::Name(qname) => qname,
            _ => return None,
        },
        // Locals keep type hover; other expressions have no declaring item.
        _ => return None,
    };
    let def = resolve_qualified_name(module, module_path, registry, qname)?;
    // The defining module's AST: the current module when the definition is
    // local, else the origin module from the registry.
    let origin = match &def.module {
        Some(path) => &registry.get(path)?.module,
        None => module,
    };
    origin
        .items
        .iter()
        .find(|item| item_name_span(item) == Some(def.span))
}

/// Find the enum-variant pattern whose name span contains `offset`, scanning
/// function and const bodies (the only items with expression bodies, matching
/// [`find_expr_at_offset`]'s reach). Patterns live only in match arms.
fn find_variant_pattern_at_offset(module: &Module, offset: u32) -> Option<&QualifiedName> {
    module.items.iter().find_map(|item| match &item.kind {
        ItemKind::Function(f) => variant_pattern_in_expr(&f.body, offset),
        ItemKind::Const(c) => variant_pattern_in_expr(&c.value, offset),
        _ => None,
    })
}

/// Recurse through an expression to every match arm, returning the variant
/// pattern whose name span contains `offset`.
fn variant_pattern_in_expr(expr: &Expr, offset: u32) -> Option<&QualifiedName> {
    match &expr.kind {
        ExprKind::Match(scrutinee, arms) => {
            variant_pattern_in_expr(scrutinee, offset).or_else(|| {
                arms.iter().find_map(|arm| {
                    variant_pattern_in_pattern(&arm.pattern, offset)
                        .or_else(|| variant_pattern_in_expr(&arm.body, offset))
                })
            })
        }
        ExprKind::Block(stmts, result) => stmts
            .iter()
            .find_map(|stmt| match &stmt.kind {
                StmtKind::Let(b) => variant_pattern_in_expr(&b.init, offset),
                StmtKind::Const(c) => variant_pattern_in_expr(&c.value, offset),
                StmtKind::Expr(e) => variant_pattern_in_expr(e, offset),
                StmtKind::Use(_) => None,
            })
            .or_else(|| {
                result
                    .as_ref()
                    .and_then(|e| variant_pattern_in_expr(e, offset))
            }),
        ExprKind::Lambda(lambda) => variant_pattern_in_expr(&lambda.body, offset),
        ExprKind::If(c, t, e) => variant_pattern_in_expr(c, offset)
            .or_else(|| variant_pattern_in_expr(t, offset))
            .or_else(|| e.as_ref().and_then(|e| variant_pattern_in_expr(e, offset))),
        ExprKind::Binary { left, right, .. } => {
            variant_pattern_in_expr(left, offset).or_else(|| variant_pattern_in_expr(right, offset))
        }
        ExprKind::Unary(_, e)
        | ExprKind::Resume(e)
        | ExprKind::TupleIndex(e, _)
        | ExprKind::RecordField(e, _) => variant_pattern_in_expr(e, offset),
        ExprKind::Sandbox(s) => variant_pattern_in_expr(&s.body, offset),
        ExprKind::Call(callee, args) => variant_pattern_in_expr(callee, offset)
            .or_else(|| args.iter().find_map(|a| variant_pattern_in_expr(a, offset))),
        ExprKind::MethodCall { receiver, args, .. } => variant_pattern_in_expr(receiver, offset)
            .or_else(|| args.iter().find_map(|a| variant_pattern_in_expr(a, offset))),
        ExprKind::Tuple(elems) | ExprKind::List(elems) => elems
            .iter()
            .find_map(|e| variant_pattern_in_expr(e, offset)),
        ExprKind::Record(fields) | ExprKind::TypedRecord { fields, .. } => fields
            .iter()
            .find_map(|(_, e)| variant_pattern_in_expr(e, offset)),
        ExprKind::Perform(call) => call
            .args
            .iter()
            .find_map(|a| variant_pattern_in_expr(a, offset)),
        ExprKind::Handle(handle) => variant_pattern_in_expr(&handle.body, offset)
            .or_else(|| {
                handle
                    .handlers
                    .iter()
                    .find_map(|h| variant_pattern_in_expr(h, offset))
            })
            .or_else(|| {
                handle
                    .else_clause
                    .as_ref()
                    .and_then(|e| variant_pattern_in_expr(e, offset))
            }),
        _ => None,
    }
}

/// Descend a pattern to the variant whose name span contains `offset`.
fn variant_pattern_in_pattern(pattern: &Pattern, offset: u32) -> Option<&QualifiedName> {
    match &pattern.kind {
        PatternKind::Variant(qname, payload) => {
            let span = qname.name_span.unwrap_or(pattern.span);
            if offset >= span.start && offset <= span.end {
                return Some(qname);
            }
            payload
                .as_ref()
                .and_then(|p| variant_pattern_in_pattern(p, offset))
        }
        PatternKind::Tuple(elems) => elems
            .iter()
            .find_map(|p| variant_pattern_in_pattern(p, offset)),
        PatternKind::Record(fields) => fields
            .iter()
            .find_map(|(_, p)| variant_pattern_in_pattern(p, offset)),
        _ => None,
    }
}

/// Resolve a (possibly qualified) name to its defining item.
///
/// Resolution order mirrors the checker: local items shadow imports; a
/// qualified path resolves its head through the module's imports, then
/// the symbol through the registry (which follows `pub use` chains to the
/// origin).
#[must_use]
pub fn resolve_qualified_name(
    module: &Module,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    qname: &QualifiedName,
) -> Option<Definition> {
    // Prefer the engine's canonical resolution when the resolve pass filled it
    // in. This is the sole path for a bare prelude-injected enum variant
    // (`Some`, `Ok`): the prelude excludes variants from `resolve_imports`
    // (module_registry/imports.rs), so the spelling-based walk below can't see
    // them, yet the resolve pass resolves them straight off the prelude scope.
    if let Some(item_ref) = item_ref_from_resolution(registry, qname)
        && let Some(def) = definition_from_item_ref(module_path, registry, &item_ref)
    {
        return Some(def);
    }

    if qname.path.is_empty() {
        // Bare name: local item first, then item imports.
        if let Some(def) = find_local_item(module, &qname.name) {
            return Some(def);
        }
        return resolve_symbol_through_imports(module_path, registry, &qname.name);
    }

    // Qualified: resolve the module the head names, then look the symbol
    // up in it. The head is either an imported module alias or the first
    // segment of an absolute path (pkg/core handled upstream by
    // the parser as prefixes on `use`; in expression position the head is
    // an alias bound by a `use`).
    if let Some(target) = resolve_module_reference(module_path, registry, &qname.path)
        && let Ok((export, origin)) = registry.lookup_symbol(&target, &qname.name)
    {
        return Some(Definition {
            span: export.name_span,
            module: Some(origin),
        });
    }

    // The explicit-enum variant spelling `Enum::Variant` / `m::Enum::Variant`,
    // where the last *path* segment names the enum rather than a module, so
    // the module-path resolution above fails. `resolve_item_ref` canonicalizes
    // every variant spelling to its `[Enum, Variant]` identity; map that to
    // the variant's declaration span so goto-definition lands on the variant.
    let item_ref = resolve_item_ref(module, module_path, registry, qname)?;
    definition_from_item_ref(module_path, registry, &item_ref)
}

/// Turn a resolved [`ItemRef`] into a navigable [`Definition`] by locating the
/// referenced item — or enum variant — in its origin module's AST. The origin
/// is `None` (current-module) when it is the module being queried, matching
/// the convention `find_local_item` uses.
fn definition_from_item_ref(
    current: &ModulePath,
    registry: &ModuleRegistry,
    item_ref: &ItemRef,
) -> Option<Definition> {
    let origin_module = &registry.get(&item_ref.module)?.module;
    let module = (&item_ref.module != current).then(|| item_ref.module.clone());
    // A two-segment ident is a variant (`[Enum, Variant]`); anything else is
    // an ordinary item keyed by its single name.
    if let [enum_name, variant_name] = item_ref.ident.as_slice() {
        for item in &origin_module.items {
            if let ItemKind::Enum(e) = &item.kind
                && e.name.as_ref() == enum_name.as_ref()
                && let Some(variant) = e.variants.iter().find(|v| v.name == *variant_name)
            {
                return Some(Definition {
                    span: variant.span,
                    module,
                });
            }
        }
        return None;
    }
    let name = item_ref.ident.first()?;
    for item in &origin_module.items {
        if item_name(item).is_some_and(|n| n.as_ref() == name.as_ref()) {
            let span = item_name_span(item).unwrap_or(item.span);
            return Some(Definition { span, module });
        }
    }
    None
}

/// A local item defined in this module.
fn find_local_item(module: &Module, name: &str) -> Option<Definition> {
    for item in &module.items {
        if item_name(item).is_some_and(|n| n.as_ref() == name) {
            let span = item_name_span(item).unwrap_or(item.span);
            return Some(Definition::local(span));
        }
        // Enum variants are in scope in the declaring module.
        if let ItemKind::Enum(e) = &item.kind
            && let Some(variant) = e.variants.iter().find(|v| v.name.as_ref() == name)
        {
            return Some(Definition::local(variant.span));
        }
    }
    None
}

/// Resolve a bare symbol through this module's imports.
fn resolve_symbol_through_imports(
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    name: &str,
) -> Option<Definition> {
    let imports = registry.resolve_imports(module_path).ok()?;
    for import in imports.imports.get(name)? {
        if let ResolvedImport::Symbol { from_module, .. } = import {
            let (export, origin) = registry.lookup_symbol(from_module, name).ok()?;
            return Some(Definition {
                span: export.name_span,
                module: Some(origin),
            });
        }
    }
    None
}

/// A resolved reference's identity input for the occurrence index: the
/// origin module plus the item's ident path — `[name]` for an ordinary item,
/// `[Enum, Variant]` for an enum variant. This mirrors the [`Fqn`] ident the
/// engine's resolve pass mints for every spelling of a name
/// (`ambient_engine::resolve`), so a variant's declaration, constructors,
/// patterns, and imports all collapse onto one identity.
///
/// [`Fqn`]: ambient_engine::fqn::Fqn
#[derive(Debug, Clone)]
pub struct ItemRef {
    /// The module that defines the referenced item. For a variant this is its
    /// declaring enum's module.
    pub module: ModulePath,
    /// The item's ident path: `[name]`, or `[Enum, Variant]` for a variant.
    pub ident: Vec<Arc<str>>,
}

impl ItemRef {
    fn item(module: ModulePath, name: Arc<str>) -> Self {
        Self {
            module,
            ident: vec![name],
        }
    }

    fn variant(module: ModulePath, enum_name: Arc<str>, variant: Arc<str>) -> Self {
        Self {
            module,
            ident: vec![enum_name, variant],
        }
    }

    /// The referenced item's simple name (the variant name for a variant) —
    /// the last ident segment.
    #[must_use]
    pub fn name(&self) -> Arc<str> {
        self.ident.last().cloned().unwrap_or_else(|| Arc::from(""))
    }
}

/// Resolve a reference to the item (or enum variant) it names, returning the
/// origin module and canonical ident path used to key the occurrence index.
///
/// This is the occurrence collector's resolver. It extends
/// [`resolve_qualified_name`] with enum-variant canonicalization: every
/// variant spelling — bare `Circle`, `Shape::Circle`, `pkg::shapes::Circle`,
/// `pkg::shapes::Shape::Circle`, or an imported `use shapes::{Circle}` — lands
/// on the two-segment ident `[Shape, Circle]` the engine's resolve pass mints
/// (`resolve/refs.rs`), so a variant's declaration and all its references
/// collapse to one `Fqn`. Ordinary items keep their single-segment `[name]`,
/// matching what a same-module `item_def` produces.
///
/// Like `resolve_qualified_name`, aliased imports (`use m::{E as F}`) are not
/// resolved — a pre-existing limitation of the analysis-layer walk that
/// affects every symbol kind, not just variants.
#[must_use]
pub fn resolve_item_ref(
    module: &Module,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    qname: &QualifiedName,
) -> Option<ItemRef> {
    // Prefer the engine's canonical resolution (bare prelude variants like
    // `Some`/`Ok` resolve only this way — see `resolve_qualified_name`).
    if let Some(item_ref) = item_ref_from_resolution(registry, qname) {
        return Some(item_ref);
    }
    if qname.path.is_empty() {
        return resolve_bare_item_ref(module, module_path, registry, &qname.name);
    }
    // A path whose prefix names a module, with the final segment an item or
    // variant that module exports (chasing `pub use` re-exports to the origin).
    if let Some(target) = resolve_module_reference(module_path, registry, &qname.path)
        && let Ok((export, origin)) = registry.lookup_symbol(&target, &qname.name)
    {
        return Some(export_item_ref(registry, &origin, export.kind, &qname.name));
    }
    // The explicit-enum spelling `Enum::Variant` / `m::Enum::Variant`, where
    // the last *path* segment names the enum rather than a module — the case
    // the engine's resolve pass leaves to the checker, but which is still a
    // deterministic name-scope resolution (the enum is spelled), landing on
    // the same `[Enum, Variant]` identity.
    resolve_explicit_variant_ref(module, module_path, registry, qname)
}

/// Turn the engine's canonical resolution — the [`Fqn`] the resolve pass wrote
/// onto a reference — into an [`ItemRef`], if present and its module is
/// registered. This is the shared "prefer engine resolution over re-derivation"
/// entry point behind both [`resolve_item_ref`] and [`resolve_qualified_name`]:
/// the resolve pass already canonicalizes every spelling (bare, qualified,
/// prelude-injected) to one `Fqn`, so consuming it is both more direct and the
/// only way to reach names the analysis-layer walk can't (bare prelude enum
/// variants, which `resolve_imports` deliberately omits).
///
/// [`Fqn`]: ambient_engine::fqn::Fqn
fn item_ref_from_resolution(registry: &ModuleRegistry, qname: &QualifiedName) -> Option<ItemRef> {
    let fqn = qname.resolved.as_ref()?;
    // Mount-aware inverse: a mounted package's module path re-attaches its
    // package-name segment.
    let module = registry.module_path_of(&fqn.module)?;
    registry.contains(&module).then(|| ItemRef {
        module,
        ident: fqn.ident.clone(),
    })
}

/// Resolve a bare (unqualified) reference — a same-module item or variant
/// (which shadow imports), then an imported symbol or variant.
fn resolve_bare_item_ref(
    module: &Module,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    name: &Arc<str>,
) -> Option<ItemRef> {
    for item in &module.items {
        if item_name(item).is_some_and(|n| n.as_ref() == name.as_ref()) {
            return Some(ItemRef::item(module_path.clone(), Arc::clone(name)));
        }
        if let ItemKind::Enum(e) = &item.kind
            && e.variants.iter().any(|v| v.name.as_ref() == name.as_ref())
        {
            return Some(ItemRef::variant(
                module_path.clone(),
                Arc::clone(&e.name),
                Arc::clone(name),
            ));
        }
    }
    let imports = registry.resolve_imports(module_path).ok()?;
    for import in imports.imports.get(name.as_ref())? {
        if let ResolvedImport::Symbol {
            from_module,
            export_kind,
            ..
        } = import
        {
            return Some(export_item_ref(registry, from_module, *export_kind, name));
        }
    }
    None
}

/// Build an [`ItemRef`] for an export of `origin` given its kind — a variant
/// lands on `[Enum, Variant]`, everything else on `[name]`.
fn export_item_ref(
    registry: &ModuleRegistry,
    origin: &ModulePath,
    kind: ExportKind,
    name: &Arc<str>,
) -> ItemRef {
    if kind == ExportKind::EnumVariant
        && let Some(enum_name) = variant_enum_name(registry, origin, name)
    {
        return ItemRef::variant(origin.clone(), enum_name, Arc::clone(name));
    }
    ItemRef::item(origin.clone(), Arc::clone(name))
}

/// Resolve the explicit-enum variant spelling where the last *path* segment
/// names the enum: `Enum::Variant` (enum in scope) or `m::Enum::Variant`
/// (enum exported by module `m`).
fn resolve_explicit_variant_ref(
    module: &Module,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    qname: &QualifiedName,
) -> Option<ItemRef> {
    let (enum_seg, prefix) = qname.path.split_last()?;
    if prefix.is_empty() {
        // `Enum::Variant`: the enum is in scope (same-module or imported).
        for item in &module.items {
            if let ItemKind::Enum(e) = &item.kind
                && e.name.as_ref() == enum_seg.as_ref()
                && e.variants
                    .iter()
                    .any(|v| v.name.as_ref() == qname.name.as_ref())
            {
                return Some(ItemRef::variant(
                    module_path.clone(),
                    Arc::clone(&e.name),
                    Arc::clone(&qname.name),
                ));
            }
        }
        let imports = registry.resolve_imports(module_path).ok()?;
        for import in imports.imports.get(enum_seg.as_ref())? {
            if let ResolvedImport::Symbol {
                from_module,
                export_kind: ExportKind::Enum,
                ..
            } = import
                && enum_has_variant(registry, from_module, enum_seg, &qname.name)
            {
                return Some(ItemRef::variant(
                    from_module.clone(),
                    Arc::clone(enum_seg),
                    Arc::clone(&qname.name),
                ));
            }
        }
        return None;
    }
    // `m::..::Enum::Variant`: the prefix names a module, `enum_seg` an enum.
    let target = resolve_module_reference(module_path, registry, prefix)?;
    let (export, origin) = registry.lookup_symbol(&target, enum_seg).ok()?;
    if export.kind == ExportKind::Enum && enum_has_variant(registry, &origin, enum_seg, &qname.name)
    {
        return Some(ItemRef::variant(
            origin,
            Arc::clone(enum_seg),
            Arc::clone(&qname.name),
        ));
    }
    None
}

/// The name of the enum in `module` that declares `variant`, if any.
fn variant_enum_name(
    registry: &ModuleRegistry,
    module: &ModulePath,
    variant: &Arc<str>,
) -> Option<Arc<str>> {
    let info = registry.get(module)?;
    info.module.items.iter().find_map(|item| match &item.kind {
        ItemKind::Enum(e)
            if e.variants
                .iter()
                .any(|v| v.name.as_ref() == variant.as_ref()) =>
        {
            Some(Arc::clone(&e.name))
        }
        _ => None,
    })
}

/// Whether `module` declares an enum named `enum_name` with variant `variant`.
fn enum_has_variant(
    registry: &ModuleRegistry,
    module: &ModulePath,
    enum_name: &str,
    variant: &str,
) -> bool {
    registry.get(module).is_some_and(|info| {
        info.module.items.iter().any(|item| {
            matches!(&item.kind,
                ItemKind::Enum(e) if e.name.as_ref() == enum_name
                    && e.variants.iter().any(|v| v.name.as_ref() == variant))
        })
    })
}

/// Find the module referenced at `offset` inside a `use` item's path.
///
/// Hovering `utils` in `use pkg::utils::helper;` resolves the partial
/// path `pkg::utils` through the registry — the same resolution imports
/// go through — and returns it when it names a registered module.
#[must_use]
pub fn find_use_module_at_offset(
    module: &Module,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    offset: u32,
) -> Option<ModulePath> {
    for item in &module.items {
        let ItemKind::Use(use_def) = &item.kind else {
            continue;
        };
        for (index, (_, span)) in use_def.path.iter().enumerate() {
            if offset >= span.start && offset < span.end {
                let names: Vec<_> = use_def.path[..=index]
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect();
                let target = registry
                    .resolve_use_path(module_path, &use_def.prefix, &names)
                    .ok()?;
                return registry.contains(&target).then_some(target);
            }
        }
    }
    None
}

/// Find the module referenced at `offset` inside a qualified name's path
/// (`utils` in `utils::helper(...)`), resolved through the registry.
#[must_use]
pub fn find_qname_module_at_offset(
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    qname: &QualifiedName,
    offset: u32,
) -> Option<ModulePath> {
    if qname.path_spans.len() != qname.path.len() {
        return None;
    }
    for (index, span) in qname.path_spans.iter().enumerate() {
        if offset >= span.start && offset < span.end {
            let target = resolve_module_reference(module_path, registry, &qname.path[..=index])?;
            return registry.contains(&target).then_some(target);
        }
    }
    None
}

/// Resolve a module reference in expression position (`alias::rest…`),
/// where `alias` was bound by a `use` (or names a `core` module qualified
/// from the root).
fn resolve_module_reference(
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    path: &[std::sync::Arc<str>],
) -> Option<ModulePath> {
    let (head, rest) = path.split_first()?;

    // Reserved roots spelled inline: `core::collections::list::…`,
    // `core::system::…`, `pkg::a::b::…` resolve absolutely, same as the
    // checker.
    let absolute = match head.as_ref() {
        "core" => {
            let mut segments = vec![head.clone()];
            segments.extend(rest.iter().cloned());
            ModulePath::from_segments(segments)
        }
        // `pkg` anchors at the package root; a workspace-rooted path
        // (`::pkg::…`, spelled with an empty head segment) is already
        // absolute in the mounted namespace.
        "pkg" | "" => ModulePath::from_segments(rest.to_vec()),
        _ => None,
    };
    if let Some(path) = absolute {
        return registry.contains(&path).then_some(path);
    }

    // Otherwise the head is an alias bound by an import.
    let imports = registry.resolve_imports(module_path).ok()?;
    let base = imports.imports.get(head.as_ref())?.iter().find_map(|i| {
        if let ResolvedImport::Module(m) = i {
            Some(m.clone())
        } else {
            None
        }
    })?;

    if rest.is_empty() {
        return Some(base);
    }
    let mut segments: Vec<_> = base.segments().to_vec();
    segments.extend(rest.iter().cloned());
    ModulePath::from_segments(segments)
}

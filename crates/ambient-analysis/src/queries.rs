//! Position-based queries over the typed AST.
//!
//! These serve IDE features (hover, go-to-definition) and the REPL. They
//! are pure AST walks — resolution against other modules goes through the
//! engine's `ModuleRegistry`, never a parallel index.

use ambient_engine::ast::{
    BindingId, Expr, ExprKind, FunctionDef, Item, ItemKind, Module, Pattern, PatternKind,
    QualifiedName, Span, StmtKind,
};
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::{ModuleRegistry, ResolvedImport};
use ambient_engine::types::Type;

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
        ItemKind::TypeAlias(t) => Some(t.name_span),
        ItemKind::Enum(e) => Some(e.name_span),
        ItemKind::Ability(a) => Some(a.name_span),
        ItemKind::Trait(t) => Some(t.name_span),
        ItemKind::Use(_) | ItemKind::Impl(_) => None,
    }
}

/// The name of an item, if it defines one.
#[must_use]
pub fn item_name(item: &Item) -> Option<&std::sync::Arc<str>> {
    match &item.kind {
        ItemKind::Function(f) => Some(&f.name),
        ItemKind::Const(c) => Some(&c.name),
        ItemKind::TypeAlias(t) => Some(&t.name),
        ItemKind::Enum(e) => Some(&e.name),
        ItemKind::Ability(a) => Some(&a.name),
        ItemKind::Trait(t) => Some(&t.name),
        ItemKind::Use(_) | ItemKind::Impl(_) => None,
    }
}

fn find_expr_in_item_at_offset(item: &ItemKind, offset: u32) -> Option<&Expr> {
    match item {
        ItemKind::Function(f) => find_expr_in_tree(&f.body, offset),
        ItemKind::Const(c) => find_expr_in_tree(&c.value, offset),
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
                    }
                }
            }
            result.as_ref().and_then(|e| find_expr_in_tree(e, offset))
        }
        ExprKind::Lambda(lambda) => find_expr_in_tree(&lambda.body, offset),
        ExprKind::Handle(handle) => find_expr_in_tree(&handle.body, offset)
            .or_else(|| {
                handle
                    .handlers
                    .iter()
                    .find_map(|h| find_expr_in_tree(&h.body, offset))
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
    ty.to_string()
}

/// Format a type for hover, surfacing the module-qualified identity of a
/// primitive (`core::String`) that its bare `Display` name (`String`) omits.
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
        None => ty.to_string(),
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
    let expr = find_expr_at_offset(module, offset)?;

    match &expr.kind {
        ExprKind::Local(binding_id) => {
            for item in &module.items {
                if let ItemKind::Function(func) = &item.kind
                    && let Some(def_span) = find_binding_definition(func, *binding_id)
                {
                    return Some(Definition::local(def_span));
                }
            }
            None
        }
        ExprKind::Name(qname) => resolve_qualified_name(module, module_path, registry, qname),
        ExprKind::Call(callee, _) => {
            if let ExprKind::Name(qname) = &callee.kind {
                resolve_qualified_name(module, module_path, registry, qname)
            } else {
                None
            }
        }
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
    if qname.path.is_empty() {
        // Bare name: local item first, then item imports.
        if let Some(def) = find_local_item(module, &qname.name) {
            return Some(def);
        }
        return resolve_symbol_through_imports(module_path, registry, &qname.name);
    }

    // Qualified: resolve the module the head names, then look the symbol
    // up in it. The head is either an imported module alias or the first
    // segment of an absolute path (pkg/core/platform handled upstream by
    // the parser as prefixes on `use`; in expression position the head is
    // an alias bound by a `use`).
    let target = resolve_module_reference(module_path, registry, &qname.path)?;
    let (export, origin) = registry.lookup_symbol(&target, &qname.name).ok()?;
    Some(Definition {
        span: export.name_span,
        module: Some(origin),
    })
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
/// where `alias` was bound by a `use` (or names a core/platform module
/// qualified from the root).
fn resolve_module_reference(
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    path: &[std::sync::Arc<str>],
) -> Option<ModulePath> {
    let (head, rest) = path.split_first()?;

    // Reserved roots spelled inline: `core::List::…`, `platform::…`,
    // `pkg::a::b::…` resolve absolutely, same as the checker.
    let absolute = match head.as_ref() {
        "core" | "platform" => {
            let mut segments = vec![head.clone()];
            segments.extend(rest.iter().cloned());
            ModulePath::from_segments(segments)
        }
        "pkg" => ModulePath::from_segments(rest.to_vec()),
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

fn find_binding_definition(func: &FunctionDef, target_id: BindingId) -> Option<Span> {
    // Check parameters.
    for param in &func.params {
        if param.id == target_id {
            return Some(param.span);
        }
    }

    // Check body.
    find_binding_in_expr(&func.body, target_id)
}

fn find_binding_in_expr(expr: &Expr, target_id: BindingId) -> Option<Span> {
    match &expr.kind {
        ExprKind::Block(stmts, result) => {
            for stmt in stmts {
                if let StmtKind::Let(binding) = &stmt.kind {
                    if binding.id == target_id {
                        return Some(stmt.span);
                    }
                    if let Some(span) = find_binding_in_expr(&binding.init, target_id) {
                        return Some(span);
                    }
                }
            }
            if let Some(result) = result {
                find_binding_in_expr(result, target_id)
            } else {
                None
            }
        }
        ExprKind::Match(scrutinee, arms) => {
            if let Some(span) = find_binding_in_expr(scrutinee, target_id) {
                return Some(span);
            }
            for arm in arms {
                if let Some(span) = find_binding_in_pattern(&arm.pattern, target_id) {
                    return Some(span);
                }
                if let Some(span) = find_binding_in_expr(&arm.body, target_id) {
                    return Some(span);
                }
            }
            None
        }
        ExprKind::Lambda(lambda) => {
            for param in &lambda.params {
                if param.id == target_id {
                    return Some(param.span);
                }
            }
            find_binding_in_expr(&lambda.body, target_id)
        }
        ExprKind::If(condition, then_branch, else_branch) => {
            find_binding_in_expr(condition, target_id)
                .or_else(|| find_binding_in_expr(then_branch, target_id))
                .or_else(|| {
                    else_branch
                        .as_ref()
                        .and_then(|e| find_binding_in_expr(e, target_id))
                })
        }
        ExprKind::Binary { left, right, .. } => {
            find_binding_in_expr(left, target_id).or_else(|| find_binding_in_expr(right, target_id))
        }
        ExprKind::Unary(_, operand) => find_binding_in_expr(operand, target_id),
        ExprKind::Call(callee, args) => find_binding_in_expr(callee, target_id)
            .or_else(|| args.iter().find_map(|a| find_binding_in_expr(a, target_id))),
        ExprKind::Tuple(elements) | ExprKind::List(elements) => elements
            .iter()
            .find_map(|e| find_binding_in_expr(e, target_id)),
        ExprKind::Record(fields) => fields
            .iter()
            .find_map(|(_, e)| find_binding_in_expr(e, target_id)),
        ExprKind::RecordField(object, _) => find_binding_in_expr(object, target_id),
        ExprKind::TupleIndex(tuple, _) => find_binding_in_expr(tuple, target_id),
        ExprKind::Handle(handle) => find_binding_in_expr(&handle.body, target_id).or_else(|| {
            handle
                .else_clause
                .as_ref()
                .and_then(|e| find_binding_in_expr(e, target_id))
        }),
        ExprKind::Sandbox(sandbox) => find_binding_in_expr(&sandbox.body, target_id),
        _ => None,
    }
}

fn find_binding_in_pattern(pattern: &Pattern, target_id: BindingId) -> Option<Span> {
    match &pattern.kind {
        PatternKind::Binding(id, _) => {
            if *id == target_id {
                Some(pattern.span)
            } else {
                None
            }
        }
        PatternKind::Tuple(elements) => elements
            .iter()
            .find_map(|p| find_binding_in_pattern(p, target_id)),
        PatternKind::Record(fields) => fields
            .iter()
            .find_map(|(_, p)| find_binding_in_pattern(p, target_id)),
        PatternKind::Variant(_, payload) => payload
            .as_ref()
            .and_then(|p| find_binding_in_pattern(p, target_id)),
        _ => None,
    }
}

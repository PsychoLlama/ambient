//! Plain-text signature and doc rendering for declarations.
//!
//! One rendering of "what is this item's signature?", shared by every
//! frontend that inspects a declaration: LSP hover (which wraps these
//! strings in markdown fences) and the REPL's inspection commands. The
//! output is plain source-shaped text — no markdown, no protocol types —
//! so each frontend stays a thin presentation layer over the same
//! derivation and can never disagree about what a signature says.
//!
//! Types render through [`format_type`]/[`format_type_hover`], the same
//! canonical `Display` the compiler's diagnostics use.

use ambient_engine::ast::{
    Expr, ExprKind, ExternFnDef, FunctionDef, Item, ItemKind, Param, RowExpr, Span, TypeParam,
};
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;
use ambient_engine::types::Type;

use super::{AssocTypeAt, format_type, format_type_hover};

/// Render an item's signature: `fn f(x: Number): Number with Log`,
/// `struct Point { … }`, `trait Step<T> { … }`, and so on for every
/// [`ItemKind`]. The item's doc string is separate — read [`Item::doc`]
/// (or [`item_doc`]) and attach it however the frontend presents docs.
#[must_use]
pub fn item_signature(item: &Item) -> String {
    let mut content = String::new();
    item_kind_signature(&item.kind, &mut content);
    content
}

/// The item's `///` documentation, if any. Companion to [`item_signature`]
/// so callers can take both halves of an item's rendered identity from one
/// module.
#[must_use]
pub fn item_doc(item: &Item) -> Option<&str> {
    item.doc.as_deref()
}

/// Append an item kind's signature to `content`.
fn item_kind_signature(kind: &ItemKind, content: &mut String) {
    match kind {
        ItemKind::Function(f) => function_signature(f, content),
        ItemKind::Const(c) => {
            content.push_str("const ");
            content.push_str(&c.name);
            // The annotation is optional; only render `: Type` when written.
            if let Some(ty) = &c.ty {
                content.push_str(": ");
                content.push_str(&format_type(ty));
            }
        }
        ItemKind::Struct(s) => {
            // A struct's body is a record — bare (`struct Foo`) or wrapped in a
            // nominal type (`unique(...) struct Foo`, which `format_type` would
            // print as just the name). Unwrap the nominal to show the fields.
            let body = match &s.ty {
                Type::Nominal(nom) => nom.inner.as_ref(),
                other => other,
            };
            content.push_str("struct ");
            content.push_str(&s.name);
            type_params_signature(&s.type_params, content);
            content.push(' ');
            content.push_str(&format_type(body));
        }
        ItemKind::TypeAlias(t) => {
            content.push_str("type ");
            content.push_str(&t.name);
            type_params_signature(&t.type_params, content);
            content.push_str(" = ");
            content.push_str(&format_type(&t.ty));
        }
        ItemKind::Set(s) => {
            content.push_str("set ");
            content.push_str(&s.name);
            content.push_str(" = ");
            content.push_str(&format_row_expr(&s.body));
        }
        ItemKind::Enum(e) => {
            content.push_str("enum ");
            content.push_str(&e.name);
            type_params_signature(&e.type_params, content);
        }
        ItemKind::Ability(a) => {
            content.push_str("ability ");
            content.push_str(&a.name);
        }
        ItemKind::Use(_) => content.push_str("use ..."),
        ItemKind::Trait(t) => {
            content.push_str("trait ");
            content.push_str(&t.name);
            type_params_signature(&t.type_params, content);
            // Associated types are the trait's type-level surface; show them
            // in the header signature (methods would bloat it).
            if !t.assoc_types.is_empty() {
                content.push_str(" {");
                for assoc in &t.assoc_types {
                    content.push_str("\n    type ");
                    content.push_str(&assoc.name);
                    content.push(';');
                }
                content.push_str("\n}");
            }
        }
        ItemKind::Impl(i) => {
            content.push_str("impl ");
            if let Some(trait_name) = &i.trait_name {
                content.push_str(&trait_name.name.name);
                content.push_str(" for ");
            }
            content.push_str(&format_type(&i.for_type));
            if !i.assoc_types.is_empty() {
                content.push_str(" {");
                for assoc in &i.assoc_types {
                    content.push_str("\n    type ");
                    content.push_str(&assoc.name);
                    content.push_str(" = ");
                    content.push_str(&format_type(&assoc.ty));
                    content.push(';');
                }
                content.push_str("\n}");
            }
        }
        ItemKind::ExternFn(e) => extern_fn_signature(e, content),
    }
}

/// Append a function's signature (`pub fn f<T>(x: T): T with Log`) to
/// `content`.
fn function_signature(f: &FunctionDef, content: &mut String) {
    if f.is_public {
        content.push_str("pub ");
    }
    content.push_str("fn ");
    content.push_str(&f.name);
    type_params_signature(&f.type_params, content);
    params_signature(&f.params, false, content);
    if let Some(ret) = &f.ret_ty {
        content.push_str(": ");
        content.push_str(&format_type(ret));
    }
    if !f.abilities.is_empty() {
        content.push_str(" with ");
        for (i, ability) in f.abilities.iter().enumerate() {
            if i > 0 {
                content.push_str(", ");
            }
            content.push_str(&ability.name);
        }
    }
}

/// Append an `extern fn`'s signature to `content`. Also used on its own for
/// document symbols, where the extern declaration *is* the whole item.
pub fn extern_fn_signature(e: &ExternFnDef, content: &mut String) {
    if e.is_public {
        content.push_str("pub ");
    }
    content.push_str("extern fn ");
    content.push_str(&e.name);
    type_params_signature(&e.type_params, content);
    params_signature(&e.params, false, content);
    content.push_str(": ");
    content.push_str(&format_type(&e.ret_ty));
}

/// Append `<A, B>` to `content` when `type_params` is non-empty.
pub fn type_params_signature(type_params: &[TypeParam], content: &mut String) {
    if !type_params.is_empty() {
        content.push('<');
        for (i, tp) in type_params.iter().enumerate() {
            if i > 0 {
                content.push_str(", ");
            }
            content.push_str(&tp.name);
        }
        content.push('>');
    }
}

/// Append a parenthesized parameter list (`(self, x: T, y)`) to `content`.
/// Unannotated parameters render bare — the annotation is what was written.
fn params_signature(params: &[Param], has_self: bool, content: &mut String) {
    content.push('(');
    let mut first = true;
    if has_self {
        content.push_str("self");
        first = false;
    }
    for param in params {
        if !first {
            content.push_str(", ");
        }
        first = false;
        content.push_str(&param.name);
        if let Some(ty) = &param.ty {
            content.push_str(": ");
            content.push_str(&format_type(ty));
        }
    }
    content.push(')');
}

/// Render the signature of an associated-type name — a trait's declaration
/// (`type Out`) or an impl's binding (`type Out = T`). The owning trait/impl
/// context is on the [`AssocTypeAt`] itself for the frontend to phrase.
#[must_use]
pub fn assoc_type_signature(assoc: &AssocTypeAt<'_>) -> String {
    match assoc {
        AssocTypeAt::TraitDecl { decl, .. } => format!("type {}", decl.name),
        AssocTypeAt::ImplBinding { binding, .. } => {
            format!("type {} = {}", binding.name, format_type(&binding.ty))
        }
    }
}

/// Render the signature of the method *declared* at `name_span` in
/// `module_path` — an impl method (`impl Trait for T { fn m … }`, or an
/// inherent `impl T`) or an ability method — reading the declaration's AST
/// from the registry. Pure rendering: the resolution (which declaration this
/// is) already happened upstream (e.g. in the occurrence index); this only
/// turns a known span into a signature. `None` when no declaration sits at
/// that span.
#[must_use]
pub fn method_signature_at(
    registry: &ModuleRegistry,
    module_path: &ModulePath,
    name_span: Span,
) -> Option<String> {
    let info = registry.get(module_path)?;
    for item in &info.module.items {
        match &item.kind {
            ItemKind::Impl(impl_def) => {
                for method in &impl_def.methods {
                    if method.name_span == name_span {
                        return Some(method_signature(
                            &method.name,
                            method.has_self,
                            &method.params,
                            method.ret_ty.as_ref(),
                        ));
                    }
                }
            }
            ItemKind::Ability(ability) => {
                for method in &ability.methods {
                    if method.name_span == name_span {
                        return Some(method_signature(
                            &method.name,
                            false,
                            &method.params,
                            Some(&method.ret_ty),
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Render a method signature: `fn name(self, p: T, …): Ret`.
fn method_signature(name: &str, has_self: bool, params: &[Param], ret_ty: Option<&Type>) -> String {
    let mut content = String::from("fn ");
    content.push_str(name);
    params_signature(params, has_self, &mut content);
    if let Some(ret) = ret_ty {
        content.push_str(": ");
        content.push_str(&format_type(ret));
    }
    content
}

/// Render a module's header line: `module pkg::utils`. The module's doc
/// string comes from [`module_doc`].
#[must_use]
pub fn module_signature(module_path: &ModulePath) -> String {
    format!("module {module_path}")
}

/// A module's `///` documentation from the registry — the same module info
/// the checker resolves imports against. `None` when the module isn't
/// registered or has no doc.
#[must_use]
pub fn module_doc<'a>(registry: &'a ModuleRegistry, module_path: &ModulePath) -> Option<&'a str> {
    registry.get(module_path)?.module.doc.as_deref()
}

/// Render an expression's inspected form: `name: Type` for names, locals,
/// literals, and record fields; just the type for anything else.
///
/// The type renders through [`format_type_hover`], so a primitive-typed
/// expression shows its fully-qualified identity (`core::primitives::string`)
/// rather than the bare `String`. The literal arms fall back to that same FQN
/// when inference hasn't attached a type, since a literal's primitive is
/// unambiguous. The label for a name/local is the *spelled* source at the
/// expression's span — never the internal `local_<id>` binding number, and
/// never a checker-rewritten dispatch symbol.
#[must_use]
pub fn expr_signature(expr: &Expr, source: &str) -> String {
    use ambient_engine::types::Primitive;
    let spelled = || source.get(expr.span.start as usize..expr.span.end as usize);
    match &expr.kind {
        ExprKind::Local(_) => {
            let type_info = expr
                .ty
                .as_ref()
                .map_or("unknown".to_string(), format_type_hover);
            let name = spelled().unwrap_or("_");
            format!("{name}: {type_info}")
        }
        ExprKind::Name(qname) => {
            let type_info = expr
                .ty
                .as_ref()
                .map_or_else(|| "unknown".to_string(), format_type_hover);
            let name = spelled().unwrap_or(&qname.name);
            format!("{name}: {type_info}")
        }
        ExprKind::Bool(b) => {
            let type_info = expr
                .ty
                .as_ref()
                .map_or_else(|| Primitive::Bool.fqn().to_string(), format_type_hover);
            format!("{b}: {type_info}")
        }
        ExprKind::Number(n) => {
            let type_info = expr
                .ty
                .as_ref()
                .map_or_else(|| Primitive::Number.fqn().to_string(), format_type_hover);
            format!("{n}: {type_info}")
        }
        ExprKind::String(s) => {
            let type_info = expr
                .ty
                .as_ref()
                .map_or_else(|| Primitive::String.fqn().to_string(), format_type_hover);
            format!("\"{s}\": {type_info}")
        }
        ExprKind::RecordField(_, field_name) => {
            let type_info = expr
                .ty
                .as_ref()
                .map_or("unknown".to_string(), format_type_hover);
            format!("{field_name}: {type_info}")
        }
        _ => expr
            .ty
            .as_ref()
            .map_or("unknown".to_string(), format_type_hover),
    }
}

/// Render a `set` body (`Stdio, FileSystem`, `Difference<A, B>`).
fn format_row_expr(row: &RowExpr) -> String {
    match row {
        RowExpr::Name(qn) => qn.joined().to_string(),
        RowExpr::Union(parts) => parts
            .iter()
            .map(format_row_expr)
            .collect::<Vec<_>>()
            .join(", "),
        RowExpr::Difference(a, b) => {
            format!("Difference<{}, {}>", format_row_expr(a), format_row_expr(b))
        }
    }
}

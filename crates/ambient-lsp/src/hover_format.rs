//! Hover and signature rendering: the markdown/`ambient`-fenced strings
//! shown for items, modules, and expressions. Split from `server.rs`; pure
//! formatting, no protocol handling.

use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;

use ambient_engine::ast::ItemKind;

use crate::analysis::{format_type, format_type_hover};

/// Format hover content for an item definition, including documentation.
pub(crate) fn format_item_hover(item: &ambient_engine::ast::Item) -> String {
    let mut content = String::new();
    content.push_str("```ambient\n");
    format_item_signature(&item.kind, &mut content);
    content.push_str("\n```");

    if let Some(doc) = &item.doc {
        content.push_str("\n\n---\n\n");
        content.push_str(doc);
    }

    content
}

/// Format an item's type signature into the given buffer.
pub(crate) fn format_item_signature(kind: &ItemKind, content: &mut String) {
    match kind {
        ItemKind::Function(f) => format_function_hover(f, content),
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
            use ambient_engine::types::Type;
            // A struct's body is a record — bare (`struct Foo`) or wrapped in a
            // nominal type (`unique(...) struct Foo`, which `format_type` would
            // print as just the name). Unwrap the nominal to show the fields.
            let body = match &s.ty {
                Type::Nominal(nom) => nom.inner.as_ref(),
                other => other,
            };
            content.push_str("struct ");
            content.push_str(&s.name);
            format_type_params(&s.type_params, content);
            content.push(' ');
            content.push_str(&format_type(body));
        }
        ItemKind::TypeAlias(t) => {
            content.push_str("type ");
            content.push_str(&t.name);
            format_type_params(&t.type_params, content);
            content.push_str(" = ");
            content.push_str(&format_type(&t.ty));
        }
        ItemKind::Enum(e) => {
            content.push_str("enum ");
            content.push_str(&e.name);
            format_type_params(&e.type_params, content);
        }
        ItemKind::Ability(a) => {
            content.push_str("ability ");
            content.push_str(&a.name);
        }
        ItemKind::Use(_) => content.push_str("use ..."),
        ItemKind::Trait(t) => {
            content.push_str("trait ");
            content.push_str(&t.name);
            format_type_params(&t.type_params, content);
        }
        ItemKind::Impl(i) => {
            content.push_str("impl ");
            if let Some(trait_name) = &i.trait_name {
                content.push_str(&trait_name.name.name);
                content.push_str(" for ");
            }
            content.push_str(&format_type(&i.for_type));
        }
        ItemKind::ExternFn(e) => format_extern_fn_hover(e, content),
    }
}

/// Format an extern fn's signature for hover.
pub(crate) fn format_extern_fn_hover(e: &ambient_engine::ast::ExternFnDef, content: &mut String) {
    if e.is_public {
        content.push_str("pub ");
    }
    content.push_str("extern fn ");
    content.push_str(&e.name);
    format_type_params(&e.type_params, content);
    content.push('(');
    for (i, param) in e.params.iter().enumerate() {
        if i > 0 {
            content.push_str(", ");
        }
        content.push_str(&param.name);
        if let Some(ty) = &param.ty {
            content.push_str(": ");
            content.push_str(&format_type(ty));
        }
    }
    content.push_str("): ");
    content.push_str(&format_type(&e.ret_ty));
}

/// Format a function's signature for hover.
pub(crate) fn format_function_hover(f: &ambient_engine::ast::FunctionDef, content: &mut String) {
    if f.is_public {
        content.push_str("pub ");
    }
    content.push_str("fn ");
    content.push_str(&f.name);
    format_type_params(&f.type_params, content);
    content.push('(');
    for (i, param) in f.params.iter().enumerate() {
        if i > 0 {
            content.push_str(", ");
        }
        content.push_str(&param.name);
        if let Some(ty) = &param.ty {
            content.push_str(": ");
            content.push_str(&format_type(ty));
        }
    }
    content.push(')');
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

/// Format type parameters if present.
pub(crate) fn format_type_params(
    type_params: &[ambient_engine::ast::TypeParam],
    content: &mut String,
) {
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

/// Format hover content for a method declaration located at `name_span` in
/// `module_path`, reading the declaration's AST from the registry.
///
/// Matches an impl method (`impl Trait for T { fn m … }` or an inherent
/// `impl T`) or an ability method by its name span. Pure rendering: the
/// resolution (which declaration this is) already happened in the occurrence
/// index; this only turns a known span into a signature.
pub(crate) fn format_method_hover(
    registry: &ModuleRegistry,
    module_path: &ModulePath,
    name_span: ambient_engine::ast::Span,
) -> Option<String> {
    let info = registry.get(module_path)?;
    for item in &info.module.items {
        match &item.kind {
            ItemKind::Impl(impl_def) => {
                for method in &impl_def.methods {
                    if method.name_span == name_span {
                        return Some(fenced_signature(|c| {
                            format_method_signature(
                                &method.name,
                                method.has_self,
                                &method.params,
                                method.ret_ty.as_ref(),
                                c,
                            );
                        }));
                    }
                }
            }
            ItemKind::Ability(ability) => {
                for method in &ability.methods {
                    if method.name_span == name_span {
                        return Some(fenced_signature(|c| {
                            format_method_signature(
                                &method.name,
                                false,
                                &method.params,
                                Some(&method.ret_ty),
                                c,
                            );
                        }));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Wrap a signature-building closure in an ```` ```ambient ```` fence.
fn fenced_signature(build: impl FnOnce(&mut String)) -> String {
    let mut content = String::from("```ambient\n");
    build(&mut content);
    content.push_str("\n```");
    content
}

/// Render a method signature: `fn name(self, p: T, …): Ret`.
fn format_method_signature(
    name: &str,
    has_self: bool,
    params: &[ambient_engine::ast::Param],
    ret_ty: Option<&ambient_engine::types::Type>,
    content: &mut String,
) {
    content.push_str("fn ");
    content.push_str(name);
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
    if let Some(ret) = ret_ty {
        content.push_str(": ");
        content.push_str(&format_type(ret));
    }
}

/// Format hover content for a module, reading path and docs from the
/// registry — the same module info the checker resolves imports against.
pub(crate) fn format_module_hover(module_path: &ModulePath, registry: &ModuleRegistry) -> String {
    let mut content = String::new();

    content.push_str("```ambient\n");
    content.push_str("module ");
    content.push_str(&module_path.to_string());
    content.push_str("\n```");

    if let Some(info) = registry.get(module_path)
        && let Some(doc) = &info.module.doc
    {
        content.push_str("\n\n---\n\n");
        content.push_str(doc);
    }

    content
}

/// Format hover content for an expression.
///
/// Renders the expression's type through [`format_type_hover`], so a
/// primitive-typed expression shows its fully-qualified identity
/// (`core::primitives::string`) rather than the bare `String`. The literal arms fall back
/// to that same FQN when inference hasn't attached a type, since a literal's
/// primitive is unambiguous.
pub(crate) fn format_expr_hover(expr: &ambient_engine::ast::Expr, source: &str) -> String {
    use ambient_engine::types::Primitive;
    match &expr.kind {
        ambient_engine::ast::ExprKind::Local(_) => {
            let type_info = expr
                .ty
                .as_ref()
                .map_or("unknown".to_string(), format_type_hover);
            // A local reference's span is the spelled variable at the use site;
            // show that, never the internal `local_<id>` binding number.
            let name = source
                .get(expr.span.start as usize..expr.span.end as usize)
                .unwrap_or("_");
            format!("```ambient\n{name}: {type_info}\n```")
        }
        ambient_engine::ast::ExprKind::Name(qname) => {
            let type_info = expr
                .ty
                .as_ref()
                .map_or_else(|| "unknown".to_string(), format_type_hover);
            // Prefer the spelled source over `qname.name`: the checker rewrites
            // an associated call's callee (`String::join`) to its bare dispatch
            // symbol, which must never surface as the hover label.
            let name = source
                .get(expr.span.start as usize..expr.span.end as usize)
                .unwrap_or(&qname.name);
            format!("```ambient\n{name}: {type_info}\n```")
        }
        ambient_engine::ast::ExprKind::Bool(b) => {
            let type_info = expr
                .ty
                .as_ref()
                .map_or_else(|| Primitive::Bool.fqn().to_string(), format_type_hover);
            format!("```ambient\n{b}: {type_info}\n```")
        }
        ambient_engine::ast::ExprKind::Number(n) => {
            let type_info = expr
                .ty
                .as_ref()
                .map_or_else(|| Primitive::Number.fqn().to_string(), format_type_hover);
            format!("```ambient\n{n}: {type_info}\n```")
        }
        ambient_engine::ast::ExprKind::String(s) => {
            let type_info = expr
                .ty
                .as_ref()
                .map_or_else(|| Primitive::String.fqn().to_string(), format_type_hover);
            format!("```ambient\n\"{s}\": {type_info}\n```")
        }
        ambient_engine::ast::ExprKind::RecordField(_, field_name) => {
            let type_info = expr
                .ty
                .as_ref()
                .map_or("unknown".to_string(), format_type_hover);
            format!("```ambient\n{field_name}: {type_info}\n```")
        }
        _ => {
            let type_info = expr
                .ty
                .as_ref()
                .map_or("unknown".to_string(), format_type_hover);
            format!("```ambient\n{type_info}\n```")
        }
    }
}

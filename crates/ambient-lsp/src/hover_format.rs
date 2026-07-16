//! Hover markdown rendering: ```` ```ambient ````-fenced signatures with
//! `---`-separated docs. Pure presentation — every signature and doc string
//! comes from `ambient_analysis::queries` (shared with the REPL's
//! inspection), so this module only decides the markdown framing.

use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;

use crate::analysis::{
    assoc_type_signature, expr_signature, format_type, item_doc, item_signature,
    method_signature_at, module_doc, module_signature,
};

/// Wrap a plain-text signature in an ```` ```ambient ```` fence.
fn fenced(signature: &str) -> String {
    format!("```ambient\n{signature}\n```")
}

/// Append a `---`-separated doc section when `doc` is present.
fn with_doc(mut content: String, doc: Option<&str>) -> String {
    if let Some(doc) = doc {
        content.push_str("\n\n---\n\n");
        content.push_str(doc);
    }
    content
}

/// Format hover content for an item definition, including documentation.
pub(crate) fn format_item_hover(item: &ambient_engine::ast::Item) -> String {
    with_doc(fenced(&item_signature(item)), item_doc(item))
}

/// Format hover content for an associated-type name — a trait's declaration
/// (`type Out;`) or an impl's binding (`type Out = T;`) — with the owning
/// trait/impl for context. Returns the content and the name span to highlight.
pub(crate) fn format_assoc_type_hover(
    assoc: &crate::analysis::AssocTypeAt<'_>,
) -> (String, ambient_engine::ast::Span) {
    use crate::analysis::AssocTypeAt;
    let signature = fenced(&assoc_type_signature(assoc));
    match assoc {
        AssocTypeAt::TraitDecl { trait_def, decl } => (
            format!(
                "{signature}\n\n---\n\nAssociated type of trait `{}`.",
                trait_def.name
            ),
            decl.name_span,
        ),
        AssocTypeAt::ImplBinding { impl_def, binding } => {
            // An inherent impl can't declare associated types (the checker
            // rejects it), but render the binding alone rather than nothing.
            let content = match &impl_def.trait_name {
                Some(trait_ref) => format!(
                    "{signature}\n\n---\n\nBinds `{}::{}` for `{}`.",
                    trait_ref.name.name,
                    binding.name,
                    format_type(&impl_def.for_type)
                ),
                None => signature,
            };
            (content, binding.name_span)
        }
    }
}

/// Format hover content for a method declaration located at `name_span` in
/// `module_path` — the plain signature from the shared analysis layer,
/// fenced for markdown.
pub(crate) fn format_method_hover(
    registry: &ModuleRegistry,
    module_path: &ModulePath,
    name_span: ambient_engine::ast::Span,
) -> Option<String> {
    method_signature_at(registry, module_path, name_span).map(|s| fenced(&s))
}

/// Format hover content for a module, reading path and docs from the
/// registry — the same module info the checker resolves imports against.
pub(crate) fn format_module_hover(module_path: &ModulePath, registry: &ModuleRegistry) -> String {
    with_doc(
        fenced(&module_signature(module_path)),
        module_doc(registry, module_path),
    )
}

/// Format hover content for an expression: the shared `name: type`
/// rendering, fenced for markdown.
pub(crate) fn format_expr_hover(expr: &ambient_engine::ast::Expr, source: &str) -> String {
    fenced(&expr_signature(expr, source))
}

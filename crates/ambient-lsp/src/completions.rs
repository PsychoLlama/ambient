//! Completion rendering for the LSP.
//!
//! *What completes* is decided by `ambient_analysis::completions` — the
//! shared, frontend-neutral pipeline the REPL's tab completion also runs.
//! This module only renders: it maps each neutral
//! [`ambient_analysis::completions::CompletionItem`] onto an
//! `lsp_types::CompletionItem` (protocol kinds, label details, snippet
//! format). Anything that decides which names exist belongs in the shared
//! layer, never here.

use ambient_analysis::completions::{self as shared, CompletionKind};
use ambient_engine::ability_resolver::AbilityResolver;
use ambient_engine::ast::Module;
use lsp_types::{CompletionItem, CompletionItemKind, CompletionItemLabelDetails, InsertTextFormat};

pub use ambient_analysis::completions::CompletionContext;

/// Generate LSP completions for a given context: the shared pipeline's
/// items, rendered into protocol types. Parameters mirror
/// [`shared::get_completions`].
#[must_use]
pub fn get_completions(
    ctx: &CompletionContext<'_>,
    source: &str,
    module: Option<&Module>,
    module_path: Option<&ambient_engine::module_path::ModulePath>,
    registry: Option<&ambient_engine::module_registry::ModuleRegistry>,
    resolver: &AbilityResolver,
) -> Vec<CompletionItem> {
    shared::get_completions(ctx, source, module, module_path, registry, resolver)
        .into_iter()
        .map(render_item)
        .collect()
}

/// Render one neutral completion item as its LSP equivalent.
fn render_item(item: shared::CompletionItem) -> CompletionItem {
    let label_details = (item.signature.is_some() || item.description.is_some()).then_some(
        CompletionItemLabelDetails {
            detail: item.signature,
            description: item.description,
        },
    );
    CompletionItem {
        label: item.label,
        kind: Some(render_kind(item.kind)),
        detail: item.detail,
        label_details,
        documentation: item.doc.map(lsp_types::Documentation::String),
        filter_text: item.filter_text,
        sort_text: item.sort_text,
        insert_text: item.insert_text,
        insert_text_format: item.insert_is_snippet.then_some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }
}

/// Map a neutral completion kind onto the protocol's vocabulary.
fn render_kind(kind: CompletionKind) -> CompletionItemKind {
    match kind {
        CompletionKind::Keyword => CompletionItemKind::KEYWORD,
        CompletionKind::Type => CompletionItemKind::TYPE_PARAMETER,
        CompletionKind::Interface => CompletionItemKind::INTERFACE,
        CompletionKind::Function => CompletionItemKind::FUNCTION,
        CompletionKind::Constant => CompletionItemKind::CONSTANT,
        CompletionKind::Struct => CompletionItemKind::STRUCT,
        CompletionKind::Enum => CompletionItemKind::ENUM,
        CompletionKind::EnumVariant => CompletionItemKind::ENUM_MEMBER,
        CompletionKind::Module => CompletionItemKind::MODULE,
        CompletionKind::Variable => CompletionItemKind::VARIABLE,
        CompletionKind::Field => CompletionItemKind::FIELD,
        CompletionKind::Method => CompletionItemKind::METHOD,
        CompletionKind::Value => CompletionItemKind::VALUE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ambient_analysis::platform_prelude_resolver;

    /// The renderer keeps snippet inserts and label details intact — the
    /// contract the editor-visible behavior tests (tests/completion.rs)
    /// exercise end to end.
    #[test]
    fn renders_ability_method_as_snippet() {
        let source = "fn foo() { Stdio::o }";
        let resolver = platform_prelude_resolver();
        let ctx = CompletionContext::new(source, 19, &resolver);
        let items = get_completions(&ctx, source, None, None, None, &resolver);
        let out = items.iter().find(|i| i.label == "out!").unwrap();
        assert_eq!(out.kind, Some(CompletionItemKind::METHOD));
        assert_eq!(out.insert_text.as_deref(), Some("out!(${1:message})"));
        assert_eq!(out.insert_text_format, Some(InsertTextFormat::SNIPPET));
        assert!(out.label_details.as_ref().unwrap().detail.is_some());
    }

    /// Items without signature/description render with no label details —
    /// exactly what the neutral item carried, nothing invented.
    #[test]
    fn renders_keyword_without_label_details() {
        let source = "le";
        let resolver = platform_prelude_resolver();
        let ctx = CompletionContext::new(source, 2, &resolver);
        let items = get_completions(&ctx, source, None, None, None, &resolver);
        let item = items.iter().find(|i| i.label == "let").unwrap();
        assert_eq!(item.kind, Some(CompletionItemKind::KEYWORD));
        assert_eq!(item.detail.as_deref(), Some("keyword"));
        assert!(item.label_details.is_none());
        assert!(item.insert_text_format.is_none());
    }
}

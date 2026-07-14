//! Completion for `core::…::` paths.
//!
//! The core hierarchy is arbitrary-depth and file-defined, so completion is
//! driven entirely by the registry — keeping the "LSP is a renderer"
//! invariant, with no LSP-private model of core.

use ambient_engine::core_library::CoreLibrary;
use ambient_engine::module_registry::ExportKind;
use lsp_types::{CompletionItem, CompletionItemKind};

/// The registry path of a core module named by its `core`-relative,
/// `::`-qualified string (`collections::list` → `core::collections::list`;
/// `""` → `core`).
fn core_module_path(relative: &str) -> Option<ambient_engine::module_path::ModulePath> {
    let mut segments: Vec<&str> = vec!["core"];
    if !relative.is_empty() {
        segments.extend(relative.split("::"));
    }
    ambient_engine::module_path::ModulePath::from_str_segments(&segments)
}

/// Whether `name` is the final segment of some registered core module
/// (`List` matches `collections::list`). Drives the "bare alias of a core
/// type-companion module" case in `CompletionContext`.
pub(crate) fn core_module_final_segment_matches(name: &str) -> bool {
    CoreLibrary::available_modules()
        .iter()
        .any(|module| module.rsplit("::").next() == Some(name))
}

/// Complete a `core::…::` path. `scope` is the path relative to `core`
/// (empty for `core::` itself).
///
/// The core hierarchy is arbitrary-depth and file-defined, so this is
/// driven entirely by the registry (keeping the "LSP is a renderer"
/// invariant — no LSP-private model of core):
///
/// - the next path segment for every child module (so a namespace like
///   `core::` or `core::collections::` completes to its members),
/// - the target module's own public exports (so a leaf like
///   `core::collections::list::` completes to its API), and
/// - the target module's re-exported names (so a directory module such as
///   `core::system` completes to `Stdio`, `FileSystem`, ... — the abilities
///   its `main.ab` re-exports and that user code spells `core::system::Stdio`).
pub(crate) fn get_core_path_completions(scope: &str, prefix: &str) -> Vec<CompletionItem> {
    let registry = ambient_analysis::core_platform_registry();
    let Some(target) = core_module_path(scope) else {
        return Vec::new();
    };
    let target_segments: Vec<&str> = target.segments().iter().map(AsRef::as_ref).collect();

    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Child modules: any registered module one segment deeper than the
    // target. These are the sub-namespaces and modules (`collections`,
    // `List`, ...) reached by walking the path further. `all_modules` yields
    // ascending module-path-string order; every match here shares the exact
    // `target::` prefix, so they already arrive sorted by their last segment —
    // no explicit sort needed.
    let children: Vec<&str> = registry
        .all_modules()
        .filter_map(|info| {
            let segments = info.path.segments();
            (segments.len() == target_segments.len() + 1
                && segments
                    .iter()
                    .zip(&target_segments)
                    .all(|(seg, want)| seg.as_ref() == *want))
            .then(|| segments.last().map(AsRef::as_ref))
            .flatten()
        })
        .collect();
    for child in children {
        if child.starts_with(prefix) && seen.insert(child.to_string()) {
            let qualified = if scope.is_empty() {
                format!("core::{child}")
            } else {
                format!("core::{scope}::{child}")
            };
            items.push(CompletionItem {
                label: child.to_string(),
                kind: Some(CompletionItemKind::MODULE),
                detail: Some(format!("core library module: {qualified}")),
                documentation: registry
                    .get(&target.child(child))
                    .and_then(|info| info.module.doc.as_ref())
                    .map(|doc| lsp_types::Documentation::String(doc.to_string())),
                ..Default::default()
            });
        }
    }

    // The target module's own public exports (straight from the registry,
    // so completion can never drift from what import resolution sees).
    if let Some(info) = registry.get(&target) {
        // Deterministic source order (export tables are hash maps).
        let mut exports: Vec<_> = info.exports.values().filter(|e| e.is_public).collect();
        exports.sort_by_key(|e| e.name_span.start);

        for export in exports {
            let Some((item_kind, detail)) = export_kind_completion(export.kind) else {
                continue;
            };
            let name = export.name.as_ref();
            if name.starts_with(prefix) && seen.insert(name.to_string()) {
                items.push(CompletionItem {
                    label: name.to_string(),
                    kind: Some(item_kind),
                    detail: Some(format!("core::{scope}::{name} ({detail})")),
                    documentation: export
                        .doc
                        .as_ref()
                        .map(|d| lsp_types::Documentation::String(d.to_string())),
                    ..Default::default()
                });
            }
        }
    }

    // Re-exported names (`pub use self::stdio::Stdio;` in a directory
    // module such as `core::system`): part of the module's importable
    // surface, so complete them too. Resolve each through `lookup_symbol`
    // so the kind matches exactly what import resolution sees — no drift.
    if let Some(info) = registry.get(&target) {
        let mut names: Vec<&str> = info
            .re_exports
            .iter()
            .filter_map(|re| re.exported_name())
            .collect();
        names.sort_unstable();
        for name in names {
            if !name.starts_with(prefix) || !seen.insert(name.to_string()) {
                continue;
            }
            let Ok((export, _origin)) = registry.lookup_symbol(&target, name) else {
                continue;
            };
            let Some((item_kind, detail)) = export_kind_completion(export.kind) else {
                continue;
            };
            items.push(CompletionItem {
                label: name.to_string(),
                kind: Some(item_kind),
                detail: Some(format!("core::{scope}::{name} ({detail})")),
                documentation: export
                    .doc
                    .as_ref()
                    .map(|d| lsp_types::Documentation::String(d.to_string())),
                ..Default::default()
            });
        }
    }

    items
}

/// The completion kind and human label for an export kind, or `None` for
/// kinds that complete through another surface (enum variants).
fn export_kind_completion(kind: ExportKind) -> Option<(CompletionItemKind, &'static str)> {
    Some(match kind {
        ExportKind::Function => (CompletionItemKind::FUNCTION, "function"),
        ExportKind::Const => (CompletionItemKind::CONSTANT, "constant"),
        ExportKind::Struct => (CompletionItemKind::STRUCT, "struct"),
        ExportKind::TypeAlias => (CompletionItemKind::TYPE_PARAMETER, "type"),
        ExportKind::Enum => (CompletionItemKind::ENUM, "enum"),
        ExportKind::Trait => (CompletionItemKind::INTERFACE, "trait"),
        ExportKind::Ability => (CompletionItemKind::INTERFACE, "ability"),
        // Variant constructors complete through their enum (and the prelude
        // ones need no qualification at all); ability methods are never
        // module-level exports (they import only through the explicit
        // `Ability::method` path shape).
        ExportKind::EnumVariant | ExportKind::AbilityMethod => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_namespace_lists_reexported_abilities() {
        // `core::system` re-exports its abilities from per-ability submodules;
        // completion must surface them the way user code spells them.
        let items = get_core_path_completions("system", "");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"Stdio"),
            "core::system:: → Stdio: {labels:?}"
        );
        assert!(
            labels.contains(&"FileSystem"),
            "core::system:: → FileSystem"
        );
    }

    #[test]
    fn system_prefix_filters_to_ability() {
        let items = get_core_path_completions("system", "Std");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["Stdio"], "core::system::Std → Stdio");
    }
}

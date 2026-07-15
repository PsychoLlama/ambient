//! Code completion support for the Ambient language.
//!
//! Provides auto-completion for:
//! - Keywords (fn, let, if, etc.)
//! - Built-in types (Number, String, Bool)
//! - Function names from the current module
//! - Local variables in scope
//! - Ability names and methods
//! - Core library modules and functions

use ambient_engine::ability_resolver::{AbilityResolver, MethodSignatureInfo};
use ambient_engine::ast::{FunctionDef, ItemKind, Module};
use ambient_engine::module_registry::ExportKind;
use ambient_parser::TokenKind;
use lsp_types::{CompletionItem, CompletionItemKind, CompletionItemLabelDetails, InsertTextFormat};
use std::sync::Arc;
use uuid::Uuid;

use crate::analysis::format_type;
use crate::core_completions::{core_module_final_segment_matches, get_core_path_completions};

/// A completion context containing information about the cursor position.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct CompletionContext<'a> {
    /// The byte offset of the cursor.
    pub offset: usize,
    /// The word being typed (prefix for filtering).
    pub word_prefix: &'a str,
    /// Whether we're after a dot (field/method access).
    pub after_dot: bool,
    /// Whether we're after an ability name (for method completion).
    pub after_ability_dot: Option<&'a str>,
    /// Whether we're after a `core::…::` path (for core library
    /// completion). Holds the path *relative to* `core` — empty for
    /// `core::`, `"collections"` for `core::collections::`,
    /// `"collections::list"` for `core::collections::list::`. Generalizes
    /// over the arbitrary-depth core hierarchy.
    pub core_scope: Option<&'a str>,
    /// Whether we're after a pkg module path (for pkg module member completion).
    /// Contains the module path (e.g., "utils" for `utils::` or `utils::format` for `utils::format::`).
    pub after_pkg_module_dot: Option<&'a str>,
    /// Whether we're after a use statement prefix (pkg, core, self, super).
    pub in_use_statement: bool,
    /// Whether we're inside `unique(` for nominal type UUID completion.
    pub in_unique_paren: bool,
}

impl<'a> CompletionContext<'a> {
    /// Create a completion context from source and offset.
    ///
    /// The resolver decides which leading path segments name abilities —
    /// the same source of truth the type checker resolves performs
    /// against, so completion context can never disagree with checking.
    #[must_use]
    pub fn new(source: &'a str, offset: usize, resolver: &AbilityResolver) -> Self {
        let offset = offset.min(source.len());

        // Find the start of the current line.
        let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
        let line_prefix = &source[line_start..offset];

        // Check if we're in a use statement
        let trimmed_line = line_prefix.trim_start();
        let in_use_statement = trimmed_line.starts_with("use ");

        // Find the word being typed.
        let word_start = line_prefix
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map_or(0, |i| i + 1);
        let word_prefix = &line_prefix[word_start..];

        // Namespace / path access uses `::`; value field access uses `.`.
        let before_word = &line_prefix[..word_start];
        let trimmed_before = before_word.trim_end();
        let after_dot = trimmed_before.ends_with('.');
        let after_scope = trimmed_before.ends_with("::");

        // The qualified path immediately before a trailing `::`
        // (e.g. `core`, `core::collections::list`, `core::system::Stdio`).
        let scope_path = if after_scope {
            let without_sep = trimmed_before.strip_suffix("::").unwrap_or(trimmed_before);
            let start = without_sep
                .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != ':')
                .map_or(0, |i| i + 1);
            Some(&without_sep[start..])
        } else {
            None
        };

        // Core library completions: `core::` and any `core::<ns>::` offer
        // the next path segment; a leaf like `core::collections::list::`
        // offers that module's members. `core_scope` holds the path
        // relative to `core` (empty for `core::` itself).
        let core_scope = match scope_path {
            Some("core") => Some(""),
            Some(path) => path.strip_prefix("core::"),
            None => None,
        };

        // Ability method completion (`Stdio::`, `core::system::Stdio::`) and
        // pkg module member completion (`utils::`).
        let (after_ability_dot, after_pkg_module_dot) = match scope_path {
            Some(path) if core_scope.is_none() => {
                let last = path.rsplit("::").next().unwrap_or(path);
                if resolver.name_to_id(last).is_some() {
                    (Some(last), None)
                } else if path.chars().next().is_some_and(char::is_lowercase)
                    || core_module_final_segment_matches(last)
                {
                    // A module path: either a conventional lowercase namespace,
                    // or an alias of a known core module. The latter matters for
                    // the PascalCase type-companion modules (`List`, `Option`,
                    // `Result`): after `use core::collections::list;`, a bare `List::`
                    // would otherwise be misread as an ability and lose completions.
                    (None, Some(path))
                } else {
                    (None, None)
                }
            }
            _ => (None, None),
        };

        // Check if we're inside `unique(` for nominal type UUID completion.
        // Look for "unique(" before the cursor on the current line, with no closing paren.
        let in_unique_paren = if let Some(unique_pos) = line_prefix.rfind("unique(") {
            // Check there's no closing paren after the opening paren
            let after_open = &line_prefix[unique_pos + 7..];
            !after_open.contains(')')
        } else {
            false
        };

        Self {
            offset,
            word_prefix,
            after_dot,
            after_ability_dot,
            core_scope,
            after_pkg_module_dot,
            in_use_statement,
            in_unique_paren,
        }
    }
}

/// Generate completions for a given context.
///
/// `registry` supplies pkg-module member completions; it is the same
/// registry the document was checked against, rebuilt on every edit, so
/// module members never go stale. `source` is the document text `ctx` was
/// built from; `module_path` is the document's module identity in that
/// registry (for cross-module member resolution).
#[must_use]
pub fn get_completions(
    ctx: &CompletionContext<'_>,
    source: &str,
    module: Option<&Module>,
    module_path: Option<&ambient_engine::module_path::ModulePath>,
    registry: Option<&ambient_engine::module_registry::ModuleRegistry>,
    resolver: &AbilityResolver,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // If we're inside unique(), offer a generated UUID
    if ctx.in_unique_paren {
        items.push(get_uuid_completion());
        return items;
    }

    // If we're completing a core library path (after `core::…::`), offer
    // the next segment (for a namespace) or the module's members (for a
    // leaf), driven entirely by the registry.
    if let Some(core_scope) = ctx.core_scope {
        items.extend(get_core_path_completions(core_scope, ctx.word_prefix));
        return items;
    }

    // If we're completing an ability method, show ability methods.
    if let Some(ability_name) = ctx.after_ability_dot {
        items.extend(get_ability_method_completions(
            resolver,
            ability_name,
            ctx.word_prefix,
        ));
        return items;
    }

    // If we're completing pkg module members (after "module_name::")
    if let Some(module_path) = ctx.after_pkg_module_dot {
        // An ability namespace (`core::system::`) completes the bare names of
        // its abilities — the prefix is already typed.
        items.extend(get_namespace_ability_completions(
            resolver,
            module_path,
            ctx.word_prefix,
            registry.map(ambient_engine::module_registry::ModuleRegistry::workspace_name),
        ));
        if !items.is_empty() {
            return items;
        }
        if let Some(registry) = registry {
            items.extend(get_pkg_module_completions(
                registry,
                module_path,
                ctx.word_prefix,
            ));
        }
        return items;
    }

    // If we're after a dot (but not an ability, core, or module), we'd show field completions.
    // For now, we don't have enough type info at the cursor, so skip.
    if ctx.after_dot {
        return items;
    }

    // At item position inside a trait impl's body, the trait's missing
    // members (methods to implement, associated types to bind) are the
    // dominant intent: offer their stubs plus keywords, nothing else.
    if let (Some(module), Some(module_path), Some(registry)) = (module, module_path, registry) {
        let stubs = crate::member_completions::get_impl_member_completions(
            source,
            ctx.offset,
            ctx.word_prefix,
            module,
            module_path,
            registry,
        );
        if !stubs.is_empty() {
            items.extend(stubs);
            items.extend(get_keyword_completions(ctx.word_prefix));
            return items;
        }
    }

    // If we're in a use statement, add use prefix completions
    if ctx.in_use_statement {
        items.extend(get_use_prefix_completions(ctx.word_prefix));
    }

    // Add keyword completions.
    items.extend(get_keyword_completions(ctx.word_prefix));

    // Add builtin type completions.
    items.extend(get_type_completions(ctx.word_prefix));

    // Add ability completions.
    items.extend(get_ability_completions(resolver, ctx.word_prefix));

    // Add function name completions from the module.
    if let Some(module) = module {
        items.extend(get_function_completions(module, ctx.word_prefix));
        items.extend(crate::local_completions::get_local_completions(
            module,
            ctx.offset,
            ctx.word_prefix,
        ));
    }

    items
}

/// Generate a completion item with a fresh UUID for nominal struct definitions.
fn get_uuid_completion() -> CompletionItem {
    // Ambient requires UUID literals to be uppercase; `Uuid`'s Display is
    // lowercase, so uppercase the generated value before inserting it.
    let uuid = Uuid::new_v4().to_string().to_uppercase();
    CompletionItem {
        label: uuid.clone(),
        kind: Some(CompletionItemKind::VALUE),
        detail: Some("Generated UUID for nominal type".to_string()),
        insert_text: Some(uuid),
        ..Default::default()
    }
}

/// Get keyword completions.
fn get_keyword_completions(prefix: &str) -> Vec<CompletionItem> {
    TokenKind::all_keywords()
        .iter()
        .filter(|kw| kw.starts_with(prefix))
        .map(|kw| CompletionItem {
            label: (*kw).to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some("keyword".to_string()),
            ..Default::default()
        })
        .collect()
}

/// Get built-in type completions.
fn get_type_completions(prefix: &str) -> Vec<CompletionItem> {
    TokenKind::builtin_types()
        .iter()
        .filter(|ty| ty.starts_with(prefix))
        .map(|ty| CompletionItem {
            label: (*ty).to_string(),
            kind: Some(CompletionItemKind::TYPE_PARAMETER),
            detail: Some("type".to_string()),
            ..Default::default()
        })
        .collect()
}

/// Get ability completions from the resolver (builtins plus every
/// registered platform/user declaration). Namespaced abilities complete
/// with their required prefix (`core::system::Stdio`) — the only spelling
/// the checker accepts in `with` clauses and handler arms.
fn get_ability_completions(resolver: &AbilityResolver, prefix: &str) -> Vec<CompletionItem> {
    resolver
        .ability_names()
        .into_iter()
        .filter(|ab| ab.starts_with(prefix))
        .map(|ab| CompletionItem {
            label: ab.to_string(),
            kind: Some(CompletionItemKind::INTERFACE),
            detail: Some("ability".to_string()),
            ..Default::default()
        })
        .collect()
}

/// Get the bare ability names registered under an ability namespace
/// (`core::system::` → `Stdio`, `FileSystem`, ...). Empty when the path is not
/// a namespace, letting pkg-module completion take over.
fn get_namespace_ability_completions(
    resolver: &AbilityResolver,
    namespace: &str,
    prefix: &str,
    workspace: Option<&std::sync::Arc<str>>,
) -> Vec<CompletionItem> {
    // Build the namespace's `ModuleId` from its dotted spelling. Platform
    // abilities live under `core::system` (`Builtin`); a user namespace is
    // scoped under the analysis package name (mirroring
    // `AnalysisPackage::build_registry`), so its `ModuleId` matches the
    // registered ability namespaces. Absent a registry, fall back to the
    // empty workspace.
    let workspace = workspace
        .cloned()
        .unwrap_or_else(|| std::sync::Arc::from(""));
    let segments: Vec<&str> = namespace.split("::").collect();
    let module_id = ambient_engine::fqn::ModuleId::from_dotted_segments(&segments, &workspace);
    resolver
        .namespace_ability_names(&module_id)
        .into_iter()
        .filter(|ab| ab.starts_with(prefix))
        .map(|ab| CompletionItem {
            label: ab.to_string(),
            kind: Some(CompletionItemKind::INTERFACE),
            detail: Some(format!("ability ({namespace})")),
            ..Default::default()
        })
        .collect()
}

/// Get completions for pkg module members from the registry's export
/// tables — the same data import resolution reads, refreshed per edit.
fn get_pkg_module_completions(
    registry: &ambient_engine::module_registry::ModuleRegistry,
    module_path: &str,
    prefix: &str,
) -> Vec<CompletionItem> {
    let segments: Vec<Arc<str>> = module_path.split("::").map(Arc::from).collect();
    let direct = ambient_engine::module_path::ModulePath::from_segments(segments.clone())
        .and_then(|path| registry.get(&path));
    // The path may be an alias whose bare name matches the module's final
    // segment (`use pkg::a::b;` binds `b`). Fall back to a suffix match
    // against registered modules.
    let info = direct.or_else(|| {
        registry
            .all_modules()
            .find(|info| info.path.segments().ends_with(&segments))
    });
    let Some(info) = info else {
        return Vec::new();
    };

    let mut exports: Vec<_> = info.exports.values().filter(|e| e.is_public).collect();
    exports.sort_by_key(|e| e.name_span.start);

    exports
        .into_iter()
        .filter_map(|export| {
            let name = export.name.as_ref();
            if !name.starts_with(prefix) {
                return None;
            }

            let item_kind = match export.kind {
                ExportKind::Function => CompletionItemKind::FUNCTION,
                ExportKind::Const => CompletionItemKind::CONSTANT,
                ExportKind::Struct => CompletionItemKind::STRUCT,
                ExportKind::TypeAlias => CompletionItemKind::TYPE_PARAMETER,
                ExportKind::Enum => CompletionItemKind::ENUM,
                ExportKind::EnumVariant => CompletionItemKind::ENUM_MEMBER,
                ExportKind::Ability | ExportKind::Trait => CompletionItemKind::INTERFACE,
                // Never a module-level export (methods import only through
                // the explicit `Ability::method` path shape).
                ExportKind::AbilityMethod => return None,
            };

            Some(CompletionItem {
                label: name.to_string(),
                kind: Some(item_kind),
                detail: Some(format!("{module_path}::{name}")),
                label_details: Some(CompletionItemLabelDetails {
                    detail: None,
                    description: Some(module_path.to_string()),
                }),
                documentation: export
                    .doc
                    .as_ref()
                    .map(|d| lsp_types::Documentation::String(d.to_string())),
                ..Default::default()
            })
        })
        .collect()
}

/// Get use statement prefix completions (pkg, core, self, super).
fn get_use_prefix_completions(prefix: &str) -> Vec<CompletionItem> {
    let prefixes = [
        ("pkg", "Package-relative import (from package root)"),
        ("core", "Core library import"),
        ("self", "Relative import (same directory)"),
        ("super", "Parent directory import"),
    ];

    prefixes
        .iter()
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, doc)| CompletionItem {
            label: (*name).to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some("import prefix".to_string()),
            documentation: Some(lsp_types::Documentation::String((*doc).to_string())),
            ..Default::default()
        })
        .collect()
}

/// Build a snippet string for an ability method call.
///
/// For methods with no parameters: `name!()`
/// For methods with parameters: `name!(${1:param1}, ${2:param2})`.
/// Falls back to positional placeholders when parameter names are not
/// declared (builtin descriptors).
fn build_method_snippet(sig: &MethodSignatureInfo) -> String {
    if sig.params.is_empty() {
        return format!("{}!()", sig.name);
    }
    let placeholders: Vec<String> = (0..sig.params.len())
        .map(|i| {
            let name = sig
                .param_names
                .get(i)
                .map_or_else(|| format!("arg{}", i + 1), ToString::to_string);
            format!("${{{}:{}}}", i + 1, name)
        })
        .collect();
    format!("{}!({})", sig.name, placeholders.join(", "))
}

/// Render an ability method signature like `(path: string): ()`.
fn render_method_signature(sig: &MethodSignatureInfo) -> String {
    let params: Vec<String> = sig
        .params
        .iter()
        .enumerate()
        .map(|(i, ty)| match sig.param_names.get(i) {
            Some(name) => format!("{name}: {ty}"),
            None => ty.to_string(),
        })
        .collect();
    format!("({}): {}", params.join(", "), sig.ret)
}

/// Get ability method completions from the resolver's declared
/// signatures — the same interfaces the type checker resolves performs
/// against, so completions can never drift from the language.
fn get_ability_method_completions(
    resolver: &AbilityResolver,
    ability_name: &str,
    prefix: &str,
) -> Vec<CompletionItem> {
    let Some(ability_id) = resolver.name_to_id(ability_name) else {
        return Vec::new();
    };

    resolver
        .method_signatures(ability_id)
        .into_iter()
        .filter(|m| m.name.starts_with(prefix))
        .map(|m| {
            let snippet = build_method_snippet(&m);
            let signature = render_method_signature(&m);
            CompletionItem {
                // Show the method name with ! in the completion list
                label: format!("{}!", m.name),
                kind: Some(CompletionItemKind::METHOD),
                detail: Some(format!("{}!{}", m.name, signature)),
                label_details: Some(CompletionItemLabelDetails {
                    detail: Some(signature),
                    description: None,
                }),
                // Use snippet format for parameter placeholders
                insert_text: Some(snippet),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                ..Default::default()
            }
        })
        .collect()
}

/// Get function completions from the module.
fn get_function_completions(module: &Module, prefix: &str) -> Vec<CompletionItem> {
    module
        .items
        .iter()
        .filter_map(|item| {
            if let ItemKind::Function(func) = &item.kind {
                let name = func.name.as_ref();
                if name.starts_with(prefix) {
                    Some(function_to_completion(func))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect()
}

/// Convert a function definition to a completion item.
fn function_to_completion(func: &FunctionDef) -> CompletionItem {
    let name = func.name.to_string();

    // Build parameter signature.
    let params: Vec<String> = func
        .params
        .iter()
        .map(|p| {
            if let Some(ty) = &p.ty {
                format!("{}: {}", p.name, format_type(ty))
            } else {
                p.name.to_string()
            }
        })
        .collect();

    let ret = func.ret_ty.as_ref().map_or("_".to_string(), format_type);

    let signature = format!("({}): {}", params.join(", "), ret);

    let detail = if func.is_public {
        format!("pub fn {name}{signature}")
    } else {
        format!("fn {name}{signature}")
    };

    CompletionItem {
        label: name,
        kind: Some(CompletionItemKind::FUNCTION),
        detail: Some(detail),
        label_details: Some(CompletionItemLabelDetails {
            detail: Some(signature),
            description: None,
        }),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::platform_prelude_resolver;

    #[test]
    fn test_completion_context_simple() {
        let source = "fn foo() { let x = 1; x }";
        let ctx = CompletionContext::new(source, 23, &platform_prelude_resolver()); // cursor at 'x' near end

        assert_eq!(ctx.word_prefix, "x");
        assert!(!ctx.after_dot);
        assert!(ctx.after_ability_dot.is_none());
        assert!(ctx.core_scope.is_none());
        assert!(!ctx.in_use_statement);
    }

    #[test]
    fn test_completion_context_after_scope() {
        let source = "Stdio::o";
        let ctx = CompletionContext::new(source, 8, &platform_prelude_resolver());

        assert_eq!(ctx.word_prefix, "o");
        assert!(!ctx.after_dot);
        assert_eq!(ctx.after_ability_dot, Some("Stdio"));
        assert!(ctx.core_scope.is_none());
    }

    #[test]
    fn test_completion_context_after_core_scope() {
        let source = "use core::ma";
        let ctx = CompletionContext::new(source, 12, &platform_prelude_resolver());

        assert_eq!(ctx.word_prefix, "ma");
        assert!(!ctx.after_dot);
        assert!(ctx.after_ability_dot.is_none());
        assert_eq!(ctx.core_scope, Some(""));
        assert!(ctx.in_use_statement);
    }

    #[test]
    fn test_completion_context_in_use_statement() {
        let source = "use pk";
        let ctx = CompletionContext::new(source, 6, &platform_prelude_resolver());

        assert_eq!(ctx.word_prefix, "pk");
        assert!(!ctx.after_dot);
        assert!(ctx.in_use_statement);
    }

    #[test]
    fn test_keyword_completions() {
        let items = get_keyword_completions("le");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "let");
    }

    #[test]
    fn test_type_completions() {
        let items = get_type_completions("Num");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "Number");
    }

    #[test]
    fn test_ability_method_completions() {
        let items = get_ability_method_completions(&platform_prelude_resolver(), "Stdio", "o");
        assert_eq!(items.len(), 1); // out
        assert!(items.iter().any(|i| i.label == "out!"));

        // Check that insert_text includes the ! and snippet placeholders
        let print_item = items.iter().find(|i| i.label == "out!").unwrap();
        assert_eq!(
            print_item.insert_text.as_deref(),
            Some("out!(${1:message})")
        );
        assert_eq!(
            print_item.insert_text_format,
            Some(InsertTextFormat::SNIPPET)
        );

        // Check zero-param methods
        let random_items =
            get_ability_method_completions(&platform_prelude_resolver(), "Random", "se");
        assert_eq!(random_items.len(), 1);
        let seed_item = &random_items[0];
        assert_eq!(seed_item.label, "seed!");
        assert_eq!(seed_item.insert_text.as_deref(), Some("seed!()"));
    }

    #[test]
    fn test_empty_prefix_returns_all_keywords() {
        let items = get_keyword_completions("");
        assert_eq!(items.len(), TokenKind::all_keywords().len());
    }

    #[test]
    fn test_core_root_completions() {
        // `core::` offers the top-level namespaces and modules, not the
        // leaf types (which now live under `collections`/`primitives`).
        let items = get_core_path_completions("", "");
        for expected in ["collections", "primitives", "option", "result", "time"] {
            assert!(
                items
                    .iter()
                    .any(|i| i.label == expected && i.kind == Some(CompletionItemKind::MODULE)),
                "expected `{expected}` module at `core::`, got {:?}",
                items.iter().map(|i| &i.label).collect::<Vec<_>>()
            );
        }
        // The leaf modules are one level deeper now.
        assert!(!items.iter().any(|i| i.label == "list"));
    }

    #[test]
    fn test_core_namespace_completions() {
        // `core::collections::` walks into the namespace's children.
        let items = get_core_path_completions("collections", "");
        for expected in ["list", "map", "set"] {
            assert!(
                items.iter().any(|i| i.label == expected),
                "expected `{expected}` under `core::collections::`"
            );
        }

        // Prefix filtering: `core::primitives::num` → `number`.
        let items = get_core_path_completions("primitives", "num");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "number");
        assert_eq!(items[0].kind, Some(CompletionItemKind::MODULE));
    }

    #[test]
    fn test_core_leaf_member_completions() {
        // A leaf module offers its public members — the type it defines; the
        // low-level extern helpers are module-private and no longer surface.
        let items = get_core_path_completions("collections::list", "");
        assert!(items.iter().any(|i| i.label == "List")); // pub extern struct

        let items = get_core_path_completions("primitives::number", "Num");
        assert!(items.iter().any(|i| i.label == "Number")); // pub extern struct
    }

    #[test]
    fn test_use_prefix_completions() {
        let items = get_use_prefix_completions("c");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "core");

        let all_items = get_use_prefix_completions("");
        assert_eq!(all_items.len(), 4); // pkg, core, self, super
    }
}

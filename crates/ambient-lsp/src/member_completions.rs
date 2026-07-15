//! Trait-member stub completion inside `impl` bodies: the implemented
//! trait's unimplemented methods and unbound associated types, rendered as
//! insertable snippets. Which members are missing is decided by
//! `ambient-analysis` (`missing_impl_members_at_offset`); this module only
//! renders.

use ambient_engine::ast::{Module, TraitMethod};
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;
use lsp_types::{CompletionItem, CompletionItemKind, CompletionItemLabelDetails, InsertTextFormat};

use crate::analysis::format_type;
use crate::hover_format::format_type_params;

/// Stub completions for the trait members still missing from the impl body
/// containing `offset`. Empty when the cursor isn't at item position inside a
/// trait impl's braces, or nothing is missing.
pub(crate) fn get_impl_member_completions(
    source: &str,
    offset: usize,
    prefix: &str,
    module: &Module,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
) -> Vec<CompletionItem> {
    #[allow(clippy::cast_possible_truncation)]
    let missing = ambient_analysis::queries::missing_impl_members_at_offset(
        module,
        module_path,
        registry,
        offset as u32,
    );

    // A partial member (`fn sh`) breaks the whole impl item out of the live
    // AST — exactly when stub completion matters most. If the cursor's typed
    // word belongs to no parsed item, blank it out (offset-preserving) and
    // re-check the healed text, then ask the same question of that module.
    let healed;
    let missing = match missing {
        Some(missing) => missing,
        None => {
            let enclosed = module
                .items
                .iter()
                .any(|i| offset as u32 >= i.span.start && offset as u32 <= i.span.end);
            let Some(healed_src) = (!enclosed)
                .then(|| blank_current_member(source, offset))
                .flatten()
            else {
                return Vec::new();
            };
            let Some(module) =
                ambient_analysis::healed_module_for_completion(&healed_src, module_path, registry)
            else {
                return Vec::new();
            };
            healed = module;
            #[allow(clippy::cast_possible_truncation)]
            let Some(missing) = ambient_analysis::queries::missing_impl_members_at_offset(
                &healed,
                module_path,
                registry,
                offset as u32,
            ) else {
                return Vec::new();
            };
            missing
        }
    };

    // Item position is only *inside the braces*: the query's span check can't
    // tell the header (`impl Show for Point`) from the body, but the source can.
    let in_body = source
        .get(missing.impl_span.start as usize..offset)
        .is_some_and(|s| s.contains('{'));
    if !in_body {
        return Vec::new();
    }

    // When the line already spells the introducer (`fn sh…`, `type O…`), the
    // inserted stub must not repeat it.
    let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
    let before_word = source[line_start..offset]
        .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
        .trim_end();
    let fn_spelled = before_word.ends_with("fn");
    let type_spelled = before_word.ends_with("type");

    let mut items = Vec::new();

    for assoc in &missing.assoc_types {
        // `"type".starts_with(prefix)` keeps the stub visible while the
        // introducer itself is being typed.
        if !(assoc.name.starts_with(prefix) || "type".starts_with(prefix)) {
            continue;
        }
        let binding = format!("{} = ${{1:Type}};", assoc.name);
        items.push(CompletionItem {
            label: assoc.name.to_string(),
            kind: Some(CompletionItemKind::TYPE_PARAMETER),
            detail: Some(format!("type {} = …;", assoc.name)),
            label_details: Some(CompletionItemLabelDetails {
                detail: None,
                description: Some(format!("bind {}::{}", missing.trait_name, assoc.name)),
            }),
            filter_text: Some(format!("type {}", assoc.name)),
            sort_text: Some(format!("0_{}", assoc.name)),
            insert_text: Some(if type_spelled {
                binding
            } else {
                format!("type {binding}")
            }),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        });
    }

    for method in &missing.methods {
        if !(method.name.starts_with(prefix) || "fn".starts_with(prefix)) {
            continue;
        }
        let header = method_header(method);
        let stub = format!(
            "{} {{\n    $0\n}}",
            header.strip_prefix("fn ").unwrap_or(&header)
        );
        items.push(CompletionItem {
            label: method.name.to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some(header.clone()),
            label_details: Some(CompletionItemLabelDetails {
                detail: None,
                description: Some(format!("implement {}::{}", missing.trait_name, method.name)),
            }),
            filter_text: Some(format!("fn {}", method.name)),
            sort_text: Some(format!("0_{}", method.name)),
            insert_text: Some(if fn_spelled {
                stub
            } else {
                format!("fn {stub}")
            }),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        });
    }

    items
}

/// Blank the in-progress member text at the cursor — the current word plus a
/// directly preceding `fn`/`type` introducer — with spaces, preserving every
/// byte offset, so the enclosing impl parses again. `None` when there is
/// nothing typed at the cursor (healing can't change anything).
fn blank_current_member(source: &str, offset: usize) -> Option<String> {
    let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line = source.get(line_start..offset)?;
    let word_start = line
        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map_or(0, |i| i + 1);
    let mut blank_from = line_start + word_start;
    let before_word = line[..word_start].trim_end();
    for introducer in ["fn", "type"] {
        if before_word.ends_with(introducer) {
            blank_from = line_start + before_word.len() - introducer.len();
            break;
        }
    }
    if blank_from == offset {
        return None;
    }
    let mut healed = String::with_capacity(source.len());
    healed.push_str(&source[..blank_from]);
    healed.extend(std::iter::repeat_n(' ', offset - blank_from));
    healed.push_str(&source[offset..]);
    Some(healed)
}

/// Render a trait method's declaration header, exactly as an impl would
/// spell it: `fn name<T>(self, x: T): Ret with Ability`.
fn method_header(method: &TraitMethod) -> String {
    let mut s = String::from("fn ");
    s.push_str(&method.name);
    format_type_params(&method.type_params, &mut s);
    s.push('(');
    let mut first = true;
    if method.has_self {
        s.push_str("self");
        first = false;
    }
    for (name, ty) in &method.params {
        if !first {
            s.push_str(", ");
        }
        first = false;
        s.push_str(name);
        s.push_str(": ");
        s.push_str(&format_type(ty));
    }
    s.push_str("): ");
    s.push_str(&format_type(&method.ret_ty));
    if !method.abilities.is_empty() {
        s.push_str(" with ");
        let names: Vec<_> = method
            .abilities
            .iter()
            .map(|a| a.name.to_string())
            .collect();
        s.push_str(&names.join(", "));
    }
    s
}

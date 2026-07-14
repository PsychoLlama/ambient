//! Rename and prepare-rename request handlers.
//!
//! Both read the occurrence index (the source of exact reference ranges) and
//! rewrite every occurrence of the symbol under the cursor, rejecting anything
//! whose references the index does not fully capture. Split out of `server.rs`
//! to keep that file within its line budget; the handlers stay pure functions
//! of [`ServerState`], exactly as the other request handlers are.

use std::collections::HashMap;

use lsp_server::{RequestId, Response};
use lsp_types::{
    PrepareRenameResponse, RenameParams, TextDocumentPositionParams, TextEdit, Uri, WorkspaceEdit,
};
use serde_json::Value;

use ambient_analysis::occurrences::SymbolTarget;
use ambient_analysis::queries::resolve_qualified_name;
use ambient_engine::ast::QualifiedName;
use ambient_engine::module_registry::ExportKind;

use crate::server::{ServerState, occurrence_at, range_in_file};

/// Handle prepare-rename: return the identifier range under the cursor when
/// the symbol there can be renamed, `null` otherwise (so the editor blocks
/// the rename before prompting).
pub(crate) fn handle_prepare_rename(
    id: RequestId,
    params: &TextDocumentPositionParams,
    state: &ServerState,
) -> Response {
    let uri = &params.text_document.uri;
    let position = params.position;

    let Some(doc) = state.documents.get(uri) else {
        return Response::new_ok(id, Value::Null);
    };
    let offset = doc.position_to_offset(position.line, position.character);
    #[allow(clippy::cast_possible_truncation)]
    let offset = offset as u32;

    let Some((module, occurrence)) = occurrence_at(state, uri, offset) else {
        return Response::new_ok(id, Value::Null);
    };
    if !is_renameable_target(state, &occurrence.target) {
        return Response::new_ok(id, Value::Null);
    }

    let range = range_in_file(
        state,
        &module.uri,
        occurrence.span.start as usize,
        occurrence.span.end as usize,
    );
    let response = PrepareRenameResponse::Range(range);
    Response::new_ok(id, serde_json::to_value(response).unwrap_or(Value::Null))
}

/// Handle rename: rewrite the symbol under the cursor and all its occurrences
/// to `new_name`, rejecting collisions.
// `WorkspaceEdit::changes` is keyed by `Uri`, whose interior mutability trips
// `mutable_key_type`; we only build and serialize the map, never mutate a key.
#[allow(clippy::mutable_key_type)]
pub(crate) fn handle_rename(id: RequestId, params: &RenameParams, state: &ServerState) -> Response {
    let uri = &params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;
    let new_name = params.new_name.trim();

    let Some(doc) = state.documents.get(uri) else {
        return rename_error(id, "no document to rename in");
    };
    let offset = doc.position_to_offset(position.line, position.character);
    #[allow(clippy::cast_possible_truncation)]
    let offset = offset as u32;

    let Some((_, occurrence)) = occurrence_at(state, uri, offset) else {
        return rename_error(id, "no renameable symbol at this position");
    };
    let target = occurrence.target.clone();

    if !is_renameable_target(state, &target) {
        return rename_error(id, "this symbol cannot be renamed");
    }
    if !is_valid_identifier(new_name) {
        return rename_error(id, &format!("`{new_name}` is not a valid identifier"));
    }
    // Renaming to the current name is a no-op (and would trip collision check).
    if new_name == target.name().as_ref() {
        return Response::new_ok(
            id,
            serde_json::to_value(WorkspaceEdit::default()).unwrap_or(Value::Null),
        );
    }
    if let Some(reason) = rename_collision(state, &target, new_name) {
        return rename_error(id, &reason);
    }

    // Rewrite every occurrence (definition included), grouped by file.
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for module in &state.occurrences {
        if target.is_local() && module.module_path != *target.module() {
            continue;
        }
        for occ in &module.occurrences {
            if occ.target != target {
                continue;
            }
            let range = range_in_file(
                state,
                &module.uri,
                occ.span.start as usize,
                occ.span.end as usize,
            );
            changes
                .entry(module.uri.clone())
                .or_default()
                .push(TextEdit {
                    range,
                    new_text: new_name.to_string(),
                });
        }
    }

    let edit = WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    };
    Response::new_ok(id, serde_json::to_value(edit).unwrap_or(Value::Null))
}

/// A rename request error carrying a user-facing message (LSP "request
/// failed"). The editor surfaces `message` to the user.
fn rename_error(id: RequestId, message: &str) -> Response {
    Response::new_err(id, -32803, message.to_string())
}

/// Whether `target` can be renamed (before a new name is known).
///
/// Locals are renameable (except `self`); a module item is renameable only
/// when it is a function, const, or enum variant in a package module — the
/// symbols the occurrence index captures completely (a variant's spellings all
/// collapse onto its `[Enum, Variant]` identity, distinct from the enum's).
/// Types, enums, traits, abilities, method dispatch, and core/platform items
/// are rejected: their references aren't fully indexed, so rename would break.
fn is_renameable_target(state: &ServerState, target: &SymbolTarget) -> bool {
    match target {
        SymbolTarget::Local { name, .. } => name.as_ref() != "self",
        SymbolTarget::Item { module, name, .. } => {
            let Some(package) = state.package() else {
                return false;
            };
            if !package.modules.contains_key(&module.to_string()) {
                return false;
            }
            let Some(registry) = state.registry() else {
                return false;
            };
            matches!(
                registry
                    .get(module)
                    .and_then(|info| info.exports.get(name.as_ref()))
                    .map(|export| export.kind),
                Some(ExportKind::Function | ExportKind::Const | ExportKind::EnumVariant)
            )
        }
        // Method rename is refused: a trait method rename must rewrite the
        // declaration, every impl, and every call site coherently, which the
        // dispatch-symbol-keyed index (grouped per concrete impl) does not span.
        SymbolTarget::Method { .. } => false,
    }
}

/// Reject a rename whose new name is already visible in an affected module.
///
/// Conservative by design: for every module that defines or references the
/// symbol, ask the registry whether `new_name` already resolves to a
/// module-level symbol there; for a local, additionally reject if another
/// binding in the same file already uses the name. `Some(reason)` aborts.
fn rename_collision(state: &ServerState, target: &SymbolTarget, new_name: &str) -> Option<String> {
    let (Some(package), Some(registry)) = (state.package(), state.registry()) else {
        return None;
    };
    let candidate = QualifiedName::simple(std::sync::Arc::from(new_name));

    for module in &state.occurrences {
        if target.is_local() && module.module_path != *target.module() {
            continue;
        }
        if !module.occurrences.iter().any(|o| o.target == *target) {
            continue;
        }

        if let Some(parsed) = package.modules.get(&module.module_path.to_string())
            && resolve_qualified_name(&parsed.ast, &module.module_path, registry, &candidate)
                .is_some()
        {
            return Some(format!(
                "`{new_name}` already resolves to a symbol in module `{}`",
                module.module_path
            ));
        }

        if target.is_local()
            && module.occurrences.iter().any(|o| {
                o.target.is_local() && o.target != *target && o.target.name().as_ref() == new_name
            })
        {
            return Some(format!("`{new_name}` is already bound in this scope"));
        }
    }
    None
}

/// Whether `name` is a syntactically valid Ambient identifier.
fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

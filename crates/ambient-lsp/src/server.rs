//! LSP server implementation for the Ambient language.
//!
//! The server is a renderer over `ambient-analysis`: every diagnostic,
//! definition, and symbol comes from the same pipeline `ambient check`
//! runs, with the engine's `ModuleRegistry` as the single source of
//! cross-module truth. There is deliberately no LSP-private index of
//! modules or exports.

use std::collections::HashMap;
use std::sync::Arc;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, PrepareRenameRequest,
    References, Rename, Request as _, SemanticTokensFullRequest, WorkspaceSymbolRequest,
};
use lsp_types::{
    CompletionOptions, CompletionParams, CompletionResponse, Diagnostic,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, Location, MarkedString, MarkupContent, MarkupKind, OneOf,
    PrepareRenameResponse, PublishDiagnosticsParams, ReferenceParams, RenameOptions, RenameParams,
    SemanticTokens, SemanticTokensFullOptions, SemanticTokensOptions, SemanticTokensParams,
    SemanticTokensResult, SemanticTokensServerCapabilities, ServerCapabilities, SymbolInformation,
    SymbolKind as LspSymbolKind, TextDocumentPositionParams, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextEdit, Uri, WorkDoneProgressOptions, WorkspaceEdit,
    WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use serde_json::Value;

use ambient_analysis::occurrences::{Occurrence, SymbolTarget};
use ambient_analysis::package::AnalysisPackage;
use ambient_analysis::queries::{
    find_qname_module_at_offset, find_use_module_at_offset, resolve_qualified_name,
};
use ambient_engine::ast::{ItemKind, Module, QualifiedName};
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::{ExportKind, ModuleRegistry};

use crate::analysis::{find_definition, find_expr_at_offset, find_item_at_offset, format_type};
use crate::completions::{CompletionContext, get_completions};
use crate::convert::{diagnostic_to_lsp, offset_range_to_lsp_range};
use crate::documents::DocumentStore;
use crate::hover_format::{
    format_expr_hover, format_extern_fn_hover, format_item_hover, format_module_hover,
};
use crate::semantic_tokens::{create_legend, extract_semantic_tokens};
use crate::util::{path_to_uri, uri_to_path};

/// The analysis of one open document, plus the context needed to resolve
/// names from it: its module path and the registry it was checked
/// against. Handlers never resolve through anything else.
pub(crate) struct DocumentAnalysis {
    pub(crate) result: ambient_analysis::AnalysisResult,
    pub(crate) module_path: ModulePath,
    pub(crate) registry: Arc<ModuleRegistry>,
}

/// One module's occurrence list plus where to render its spans.
///
/// References and rename both read this: the occurrence list is the source of
/// exact reference ranges, and `uri` turns a module-local span into an LSP
/// [`Location`]. Rebuilt from the package's parsed modules on every edit, so
/// results are always fresh.
pub(crate) struct ModuleOccurrences {
    pub(crate) module_path: ModulePath,
    pub(crate) uri: Uri,
    pub(crate) occurrences: Vec<Occurrence>,
}

/// Server-wide state.
pub(crate) struct ServerState {
    pub(crate) documents: DocumentStore,
    /// Per-document analyses, keyed by URI string (Uri has interior
    /// mutability, so it can't be a map key).
    pub(crate) analyses: HashMap<String, DocumentAnalysis>,
    /// The incremental analysis session for the open documents' package, if
    /// any. Owns the package, the shared registry, and the per-module check
    /// memo; a keystroke re-analyzes through it so unchanged modules replay
    /// their cached diagnostics instead of re-checking. All decisions about
    /// *what is an error* live in `ambient-analysis`; this is a renderer.
    pub(crate) session: Option<ambient_analysis::session::AnalysisSession>,
    /// Occurrence index backing find-references and rename: every definition
    /// and reference site of every symbol, with exact spans, for every module
    /// in the package (opened or not). Rebuilt on every edit alongside the
    /// registry, so it never goes stale.
    pub(crate) occurrences: Vec<ModuleOccurrences>,
    /// Ability resolver for completions/hover: the platform prelude plus
    /// builtins, the same interfaces analysis checks against.
    pub(crate) ability_resolver: ambient_engine::ability_resolver::AbilityResolver,
}

impl ServerState {
    /// The current package, if a package session is loaded.
    fn package(&self) -> Option<&AnalysisPackage> {
        self.session
            .as_ref()
            .map(ambient_analysis::session::AnalysisSession::package)
    }

    /// The current shared package registry, if a package session is loaded.
    fn registry(&self) -> Option<&Arc<ModuleRegistry>> {
        self.session
            .as_ref()
            .map(ambient_analysis::session::AnalysisSession::registry)
    }
}

/// Run the LSP server over stdio.
///
/// # Errors
///
/// Returns an error if the server fails to start or communicate with the client.
pub fn run_server() -> anyhow::Result<()> {
    let (connection, io_threads) = Connection::stdio();
    run_server_with_connection(connection)?;
    io_threads.join()?;
    Ok(())
}

/// Run the LSP server with a given connection.
///
/// This is the core server implementation that can be used with any connection type,
/// enabling both stdio (for production) and memory (for testing) connections.
///
/// # Errors
///
/// Returns an error if the server fails to communicate with the client.
#[allow(clippy::needless_pass_by_value)] // Connection is used throughout the function
pub fn run_server_with_connection(connection: Connection) -> anyhow::Result<()> {
    // Wait for initialize request.
    let (id, params) = connection.initialize_start()?;
    let (initialize_id, _initialize_params) =
        (id, serde_json::from_value::<InitializeParams>(params)?);

    // Send our capabilities.
    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            // `prepareRename` lets the editor pre-validate a rename and pick
            // the identifier range before prompting for the new name.
            prepare_provider: Some(true),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        })),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".to_string()]),
            resolve_provider: Some(false),
            ..Default::default()
        }),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: create_legend(),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: None,
                ..Default::default()
            },
        )),
        ..Default::default()
    };

    let initialize_result = InitializeResult {
        capabilities,
        server_info: Some(lsp_types::ServerInfo {
            name: "ambient-lsp".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
    };

    connection.initialize_finish(initialize_id, serde_json::to_value(initialize_result)?)?;

    // Run the main loop.
    main_loop(&connection)?;

    Ok(())
}

/// The main server loop.
fn main_loop(connection: &Connection) -> anyhow::Result<()> {
    let mut state = ServerState {
        documents: DocumentStore::new(),
        analyses: HashMap::new(),
        session: None,
        occurrences: Vec::new(),
        ability_resolver: crate::analysis::platform_prelude_resolver(),
    };

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }

                let response = handle_request(&req, &state);
                connection.sender.send(Message::Response(response))?;
            }
            Message::Notification(notif) => {
                handle_notification(&notif, &mut state, connection)?;
            }
            Message::Response(_) => {
                // We don't send requests, so we don't expect responses.
            }
        }
    }

    Ok(())
}

/// Parse request parameters, returning an error response on failure.
fn parse_params<P: serde::de::DeserializeOwned>(
    params: &Value,
    id: &RequestId,
) -> Result<P, Response> {
    serde_json::from_value(params.clone())
        .map_err(|e| Response::new_err(id.clone(), -32602, format!("Invalid params: {e}")))
}

/// Handle an incoming request.
fn handle_request(req: &Request, state: &ServerState) -> Response {
    let id = req.id.clone();

    match req.method.as_str() {
        HoverRequest::METHOD => match parse_params(&req.params, &id) {
            Ok(params) => handle_hover(id, &params, state),
            Err(e) => e,
        },
        GotoDefinition::METHOD => match parse_params(&req.params, &id) {
            Ok(params) => handle_goto_definition(id, &params, state),
            Err(e) => e,
        },
        Completion::METHOD => match parse_params(&req.params, &id) {
            Ok(params) => handle_completion(id, &params, state),
            Err(e) => e,
        },
        DocumentSymbolRequest::METHOD => match parse_params(&req.params, &id) {
            Ok(params) => handle_document_symbol(id, &params, state),
            Err(e) => e,
        },
        WorkspaceSymbolRequest::METHOD => match parse_params(&req.params, &id) {
            Ok(params) => handle_workspace_symbol(id, &params, state),
            Err(e) => e,
        },
        SemanticTokensFullRequest::METHOD => match parse_params(&req.params, &id) {
            Ok(params) => handle_semantic_tokens(id, &params, state),
            Err(e) => e,
        },
        References::METHOD => match parse_params(&req.params, &id) {
            Ok(params) => handle_references(id, &params, state),
            Err(e) => e,
        },
        PrepareRenameRequest::METHOD => match parse_params(&req.params, &id) {
            Ok(params) => handle_prepare_rename(id, &params, state),
            Err(e) => e,
        },
        Rename::METHOD => match parse_params(&req.params, &id) {
            Ok(params) => handle_rename(id, &params, state),
            Err(e) => e,
        },
        _ => Response::new_err(id, -32601, format!("Unknown method: {}", req.method)),
    }
}

/// The URI for a module of the current package.
pub(crate) fn module_uri(
    package: Option<&AnalysisPackage>,
    module_path: &ModulePath,
) -> Option<Uri> {
    let package = package?;
    // Only package modules have files; core/platform modules are embedded.
    package
        .modules
        .contains_key(&module_path.to_string())
        .then(|| path_to_uri(&package.file_for_module(module_path)))?
}

/// Compute an LSP range in a possibly-unopened file: use the open
/// document when available, otherwise read the file from disk.
fn range_in_file(
    documents: &DocumentStore,
    uri: &Uri,
    start: usize,
    end: usize,
) -> lsp_types::Range {
    if let Some(doc) = documents.get(uri) {
        return offset_range_to_lsp_range(doc, start, end);
    }
    if let Some(file_path) = uri_to_path(uri)
        && let Ok(content) = std::fs::read_to_string(&file_path)
    {
        let temp_doc = crate::documents::Document::new(uri.clone(), 0, content);
        return offset_range_to_lsp_range(&temp_doc, start, end);
    }
    lsp_types::Range::default()
}

/// Handle hover request.
fn handle_hover(id: RequestId, params: &HoverParams, state: &ServerState) -> Response {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;

    let Some(doc) = state.documents.get(uri) else {
        return Response::new_ok(id, Value::Null);
    };

    let Some(analysis) = state.analyses.get(uri.as_str()) else {
        return Response::new_ok(id, Value::Null);
    };
    let module = &analysis.result.module;
    let registry = &analysis.registry;

    let offset = doc.position_to_offset(position.line, position.character);

    #[allow(clippy::cast_possible_truncation)]
    let offset = offset as u32;

    // First, check if hovering over a module path in a use statement.
    if let Some(target) = find_use_module_at_offset(module, &analysis.module_path, registry, offset)
    {
        let content = format_module_hover(&target, registry);
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: content,
            }),
            range: None,
        };
        return Response::new_ok(id, serde_json::to_value(hover).unwrap_or(Value::Null));
    }

    // Next, try to find an item definition at this position (hovering over a name).
    if let Some(item) = find_item_at_offset(module, offset) {
        let content = format_item_hover(item);
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: content,
            }),
            range: Some(offset_range_to_lsp_range(
                doc,
                item.span.start as usize,
                item.span.end as usize,
            )),
        };
        return Response::new_ok(id, serde_json::to_value(hover).unwrap_or(Value::Null));
    }

    // Fall back to expression-level hover.
    let Some(expr) = find_expr_at_offset(module, offset) else {
        return Response::new_ok(id, Value::Null);
    };

    // Check if hovering over a path segment in a qualified name expression.
    if let ambient_engine::ast::ExprKind::Name(qname) = &expr.kind
        && let Some(target) =
            find_qname_module_at_offset(&analysis.module_path, registry, qname, offset)
    {
        let content = format_module_hover(&target, registry);
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: content,
            }),
            range: None,
        };
        return Response::new_ok(id, serde_json::to_value(hover).unwrap_or(Value::Null));
    }

    // Build hover content based on expression kind.
    let content = format_expr_hover(expr);

    let hover = Hover {
        contents: HoverContents::Scalar(MarkedString::String(content)),
        range: Some(offset_range_to_lsp_range(
            doc,
            expr.span.start as usize,
            expr.span.end as usize,
        )),
    };

    Response::new_ok(id, serde_json::to_value(hover).unwrap_or(Value::Null))
}

/// Handle goto definition request.
fn handle_goto_definition(
    id: RequestId,
    params: &GotoDefinitionParams,
    state: &ServerState,
) -> Response {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;

    let Some(doc) = state.documents.get(uri) else {
        return Response::new_ok(id, Value::Null);
    };

    let Some(analysis) = state.analyses.get(uri.as_str()) else {
        return Response::new_ok(id, Value::Null);
    };

    let offset = doc.position_to_offset(position.line, position.character);

    #[allow(clippy::cast_possible_truncation)]
    let Some(definition) = find_definition(
        &analysis.result.module,
        &analysis.module_path,
        &analysis.registry,
        offset as u32,
    ) else {
        return Response::new_ok(id, Value::Null);
    };

    // A definition in another module needs a file to point at; core and
    // platform modules are embedded in the binary and have none.
    let target_uri = match &definition.module {
        Some(module_path) if *module_path != analysis.module_path => {
            match module_uri(state.package(), module_path) {
                Some(target) => target,
                None => return Response::new_ok(id, Value::Null),
            }
        }
        _ => uri.clone(),
    };

    let range = range_in_file(
        &state.documents,
        &target_uri,
        definition.span.start as usize,
        definition.span.end as usize,
    );

    let location = Location {
        uri: target_uri,
        range,
    };

    let response = GotoDefinitionResponse::Scalar(location);
    Response::new_ok(id, serde_json::to_value(response).unwrap_or(Value::Null))
}

/// The occurrence (and the module it lives in) under `offset` in `uri`.
///
/// Occurrences are leaf identifiers and never nest, so the first span that
/// contains the offset is the answer.
fn occurrence_at<'a>(
    state: &'a ServerState,
    uri: &Uri,
    offset: u32,
) -> Option<(&'a ModuleOccurrences, &'a Occurrence)> {
    let module = state
        .occurrences
        .iter()
        .find(|m| m.uri.as_str() == uri.as_str())?;
    let occurrence = module
        .occurrences
        .iter()
        .find(|o| offset >= o.span.start && offset <= o.span.end)?;
    Some((module, occurrence))
}

/// Every occurrence of `target` across the package, as LSP locations.
///
/// An `Item` is visible package-wide, so every module is scanned; a `Local`
/// is same-file only. With `include_declaration` false, the symbol's own
/// definition site is dropped (LSP "references excluding the declaration").
fn gather_locations(
    state: &ServerState,
    target: &SymbolTarget,
    include_declaration: bool,
) -> Vec<Location> {
    let mut locations = Vec::new();
    for module in &state.occurrences {
        if target.is_local() && module.module_path != *target.module() {
            continue;
        }
        for occ in &module.occurrences {
            if occ.target != *target || (occ.is_definition && !include_declaration) {
                continue;
            }
            let range = range_in_file(
                &state.documents,
                &module.uri,
                occ.span.start as usize,
                occ.span.end as usize,
            );
            locations.push(Location {
                uri: module.uri.clone(),
                range,
            });
        }
    }
    locations
}

/// Handle find references request.
///
/// Resolves the symbol under the cursor from the occurrence index and returns
/// every occurrence's exact range — fresh after every edit and including
/// references in files that were never opened in the editor.
fn handle_references(id: RequestId, params: &ReferenceParams, state: &ServerState) -> Response {
    let uri = &params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;

    let Some(doc) = state.documents.get(uri) else {
        return Response::new_ok(id, Value::Array(vec![]));
    };

    let offset = doc.position_to_offset(position.line, position.character);
    #[allow(clippy::cast_possible_truncation)]
    let offset = offset as u32;

    let Some((_, occurrence)) = occurrence_at(state, uri, offset) else {
        return Response::new_ok(id, Value::Array(vec![]));
    };

    let locations = gather_locations(
        state,
        &occurrence.target,
        params.context.include_declaration,
    );
    Response::new_ok(id, serde_json::to_value(locations).unwrap_or(Value::Null))
}

/// Handle prepare-rename: return the identifier range under the cursor when
/// the symbol there can be renamed, `null` otherwise (so the editor blocks
/// the rename before prompting).
fn handle_prepare_rename(
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
        &state.documents,
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
fn handle_rename(id: RequestId, params: &RenameParams, state: &ServerState) -> Response {
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
                &state.documents,
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

/// Handle completion request.
fn handle_completion(id: RequestId, params: &CompletionParams, state: &ServerState) -> Response {
    let uri = &params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;

    let Some(doc) = state.documents.get(uri) else {
        return Response::new_ok(id, Value::Null);
    };

    let offset = doc.position_to_offset(position.line, position.character);

    // Get the module from the analysis cache (if available).
    let module = state
        .analyses
        .get(uri.as_str())
        .map(|analysis| &analysis.result.module);

    // Create completion context and get completions. Module-member
    // completions read the same registry the document was checked
    // against, so they refresh with every edit.
    let registry = state
        .analyses
        .get(uri.as_str())
        .map(|analysis| analysis.registry.as_ref());
    let ctx = CompletionContext::new(&doc.text, offset, &state.ability_resolver);
    let items = get_completions(&ctx, module, registry, &state.ability_resolver);

    let response = CompletionResponse::Array(items);
    Response::new_ok(id, serde_json::to_value(response).unwrap_or(Value::Null))
}

/// Handle document symbol request.
fn handle_document_symbol(
    id: RequestId,
    params: &DocumentSymbolParams,
    state: &ServerState,
) -> Response {
    let uri = &params.text_document.uri;

    let Some(doc) = state.documents.get(uri) else {
        return Response::new_ok(id, Value::Null);
    };

    let Some(analysis) = state.analyses.get(uri.as_str()) else {
        return Response::new_ok(id, Value::Null);
    };

    let symbols = extract_document_symbols(&analysis.result.module, doc);
    let response = DocumentSymbolResponse::Nested(symbols);
    Response::new_ok(id, serde_json::to_value(response).unwrap_or(Value::Null))
}

/// Handle workspace symbol request.
///
/// Pure rendering: `ambient-analysis` decides *what symbols exist* (from the
/// live per-module structured index, plus the build snapshot for any module it
/// hasn't analyzed — live always wins). This maps each record's kind to an LSP
/// [`SymbolKind`](LspSymbolKind) and its source-relative path + span to a
/// [`Location`].
fn handle_workspace_symbol(
    id: RequestId,
    params: &WorkspaceSymbolParams,
    state: &ServerState,
) -> Response {
    let mut symbols: Vec<SymbolInformation> = Vec::new();

    if let Some(session) = state.session.as_ref() {
        let src_dir = &session.package().src_dir;
        for sym in session.workspace_symbols(&params.query) {
            let Some(uri) = path_to_uri(&src_dir.join(&sym.source_path)) else {
                continue;
            };
            let range = range_in_file(
                &state.documents,
                &uri,
                sym.span.0 as usize,
                sym.span.1 as usize,
            );

            #[allow(deprecated)] // SymbolInformation::deprecated field is deprecated
            symbols.push(SymbolInformation {
                name: sym.name,
                kind: item_kind_to_lsp(sym.kind),
                tags: None,
                deprecated: None,
                location: Location { uri, range },
                container_name: Some(sym.module),
            });
        }
    }

    let response = WorkspaceSymbolResponse::Flat(symbols);
    Response::new_ok(id, serde_json::to_value(response).unwrap_or(Value::Null))
}

/// Map an analysis-layer [`ItemKindTag`] to an LSP symbol kind.
fn item_kind_to_lsp(kind: ambient_engine::module_interface::ItemKindTag) -> LspSymbolKind {
    use ambient_engine::module_interface::ItemKindTag;
    match kind {
        ItemKindTag::Function => LspSymbolKind::FUNCTION,
        ItemKindTag::Const => LspSymbolKind::CONSTANT,
        ItemKindTag::Struct => LspSymbolKind::STRUCT,
        ItemKindTag::Enum => LspSymbolKind::ENUM,
        ItemKindTag::Alias => LspSymbolKind::TYPE_PARAMETER,
        ItemKindTag::Trait | ItemKindTag::Ability => LspSymbolKind::INTERFACE,
    }
}

/// Handle semantic tokens request.
fn handle_semantic_tokens(
    id: RequestId,
    params: &SemanticTokensParams,
    state: &ServerState,
) -> Response {
    let uri = &params.text_document.uri;

    let Some(doc) = state.documents.get(uri) else {
        return Response::new_ok(id, Value::Null);
    };

    let Some(analysis) = state.analyses.get(uri.as_str()) else {
        return Response::new_ok(id, Value::Null);
    };

    let tokens = extract_semantic_tokens(&analysis.result.module, doc);
    let result = SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data: tokens,
    });
    Response::new_ok(id, serde_json::to_value(result).unwrap_or(Value::Null))
}

/// Extract document symbols from an AST module.
fn extract_document_symbols(
    module: &Module,
    doc: &crate::documents::Document,
) -> Vec<DocumentSymbol> {
    module
        .items
        .iter()
        .filter_map(|item| item_to_document_symbol(item, doc))
        .collect()
}

/// Convert a single AST item to a document symbol.
fn item_to_document_symbol(
    item: &ambient_engine::ast::Item,
    doc: &crate::documents::Document,
) -> Option<DocumentSymbol> {
    let range = offset_range_to_lsp_range(doc, item.span.start as usize, item.span.end as usize);

    match &item.kind {
        ItemKind::Function(f) => Some(make_symbol(
            f.name.to_string(),
            Some(format_function_signature(f)),
            LspSymbolKind::FUNCTION,
            range,
            offset_range_to_lsp_range(doc, f.name_span.start as usize, f.name_span.end as usize),
            None,
        )),
        ItemKind::Const(c) => Some(make_symbol(
            c.name.to_string(),
            c.ty.as_ref().map(format_type),
            LspSymbolKind::CONSTANT,
            range,
            offset_range_to_lsp_range(doc, c.name_span.start as usize, c.name_span.end as usize),
            None,
        )),
        ItemKind::Struct(s) => Some(make_symbol(
            s.name.to_string(),
            None,
            LspSymbolKind::STRUCT,
            range,
            offset_range_to_lsp_range(doc, s.name_span.start as usize, s.name_span.end as usize),
            None,
        )),
        ItemKind::TypeAlias(t) => Some(make_symbol(
            t.name.to_string(),
            None,
            LspSymbolKind::TYPE_PARAMETER,
            range,
            offset_range_to_lsp_range(doc, t.name_span.start as usize, t.name_span.end as usize),
            None,
        )),
        ItemKind::Enum(e) => {
            let children = extract_enum_variants(e, doc);
            Some(make_symbol(
                e.name.to_string(),
                None,
                LspSymbolKind::ENUM,
                range,
                offset_range_to_lsp_range(
                    doc,
                    e.name_span.start as usize,
                    e.name_span.end as usize,
                ),
                children,
            ))
        }
        ItemKind::Ability(a) => {
            let children = extract_ability_methods(a, doc);
            Some(make_symbol(
                a.name.to_string(),
                None,
                LspSymbolKind::INTERFACE,
                range,
                offset_range_to_lsp_range(
                    doc,
                    a.name_span.start as usize,
                    a.name_span.end as usize,
                ),
                children,
            ))
        }
        ItemKind::Use(_) => None,
        ItemKind::Trait(t) => {
            let children = extract_trait_methods(t, doc);
            Some(make_symbol(
                t.name.to_string(),
                None,
                LspSymbolKind::INTERFACE,
                range,
                offset_range_to_lsp_range(
                    doc,
                    t.name_span.start as usize,
                    t.name_span.end as usize,
                ),
                children,
            ))
        }
        ItemKind::Impl(i) => Some(make_symbol(
            match &i.trait_name {
                Some(trait_name) => format!("impl {} for ...", trait_name.name),
                None => "impl ...".to_string(),
            },
            None,
            LspSymbolKind::CLASS,
            range,
            range,
            None,
        )),
        ItemKind::ExternFn(e) => Some(extern_fn_symbol(e, doc, range)),
    }
}

/// Document symbol for an `extern fn` declaration.
fn extern_fn_symbol(
    e: &ambient_engine::ast::ExternFnDef,
    doc: &crate::documents::Document,
    range: lsp_types::Range,
) -> DocumentSymbol {
    let mut signature = String::new();
    format_extern_fn_hover(e, &mut signature);
    make_symbol(
        e.name.to_string(),
        Some(signature),
        LspSymbolKind::FUNCTION,
        range,
        offset_range_to_lsp_range(doc, e.name_span.start as usize, e.name_span.end as usize),
        None,
    )
}

/// Extract trait methods as document symbols.
fn extract_trait_methods(
    trait_def: &ambient_engine::ast::TraitDef,
    doc: &crate::documents::Document,
) -> Option<Vec<DocumentSymbol>> {
    if trait_def.methods.is_empty() {
        return None;
    }

    let symbols: Vec<_> = trait_def
        .methods
        .iter()
        .map(|m| {
            make_symbol(
                m.name.to_string(),
                None,
                LspSymbolKind::METHOD,
                offset_range_to_lsp_range(doc, m.span.start as usize, m.span.end as usize),
                offset_range_to_lsp_range(
                    doc,
                    m.name_span.start as usize,
                    m.name_span.end as usize,
                ),
                None,
            )
        })
        .collect();

    Some(symbols)
}

/// Create a `DocumentSymbol` with the given properties.
#[allow(deprecated)]
fn make_symbol(
    name: String,
    detail: Option<String>,
    kind: LspSymbolKind,
    range: lsp_types::Range,
    selection_range: lsp_types::Range,
    children: Option<Vec<DocumentSymbol>>,
) -> DocumentSymbol {
    DocumentSymbol {
        name,
        detail,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range,
        children,
    }
}

/// Extract enum variants as child document symbols.
fn extract_enum_variants(
    e: &ambient_engine::ast::EnumDef,
    doc: &crate::documents::Document,
) -> Option<Vec<DocumentSymbol>> {
    let children: Vec<_> = e
        .variants
        .iter()
        .map(|v| {
            let r = offset_range_to_lsp_range(doc, v.span.start as usize, v.span.end as usize);
            make_symbol(
                v.name.to_string(),
                v.payload.as_ref().map(format_type),
                LspSymbolKind::ENUM_MEMBER,
                r,
                r,
                None,
            )
        })
        .collect();
    if children.is_empty() {
        None
    } else {
        Some(children)
    }
}

/// Extract ability methods as child document symbols.
fn extract_ability_methods(
    a: &ambient_engine::ast::AbilityDef,
    doc: &crate::documents::Document,
) -> Option<Vec<DocumentSymbol>> {
    let children: Vec<_> = a
        .methods
        .iter()
        .map(|m| {
            let r = offset_range_to_lsp_range(doc, m.span.start as usize, m.span.end as usize);
            make_symbol(
                m.name.to_string(),
                Some(format_ability_method_signature(m)),
                LspSymbolKind::METHOD,
                r,
                r,
                None,
            )
        })
        .collect();
    if children.is_empty() {
        None
    } else {
        Some(children)
    }
}

/// Format a function signature for display.
fn format_function_signature(f: &ambient_engine::ast::FunctionDef) -> String {
    let params: Vec<String> = f
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
    let ret = f
        .ret_ty
        .as_ref()
        .map_or(String::new(), |ty| format!(" -> {}", format_type(ty)));
    format!("fn({}){}", params.join(", "), ret)
}

/// Format an ability method signature for display.
fn format_ability_method_signature(m: &ambient_engine::ast::AbilityMethod) -> String {
    let params: Vec<String> = m
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, format_type(p.declared_ty())))
        .collect();
    format!("fn({}) -> {}", params.join(", "), format_type(&m.ret_ty))
}

/// Handle an incoming notification.
fn handle_notification(
    notif: &Notification,
    state: &mut ServerState,
    connection: &Connection,
) -> anyhow::Result<()> {
    match notif.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params: DidOpenTextDocumentParams = serde_json::from_value(notif.params.clone())?;
            let uri = params.text_document.uri.clone();
            let version = params.text_document.version;
            let text = params.text_document.text.clone();

            state.documents.open(uri.clone(), version, text);

            // Discover the package once, so cross-module analysis and the
            // occurrence index (built in `reanalyze_all`) see every module.
            // The session owns the package + registry + per-module check memo.
            if state.session.is_none()
                && let Some(file_path) = uri_to_path(&uri)
                && let Some(mut package) = AnalysisPackage::discover(&file_path)
            {
                package.load_modules();
                let mut session = ambient_analysis::session::AnalysisSession::new(package);
                // Best-effort: back workspace-symbol search with the last
                // build's snapshot index for any module not live-analyzed.
                // Absent/corrupt snapshots are a silent no-op (cold workspace).
                session.load_snapshot();
                state.session = Some(session);
            }

            crate::reanalyze::reanalyze_all(&uri, state, connection)?;
        }
        DidChangeTextDocument::METHOD => {
            let params: DidChangeTextDocumentParams = serde_json::from_value(notif.params.clone())?;
            let uri = params.text_document.uri.clone();
            let version = params.text_document.version;

            // We use full sync, so there's exactly one change with the full text.
            if let Some(change) = params.content_changes.into_iter().next() {
                state.documents.update(&uri, version, change.text);
                crate::reanalyze::reanalyze_all(&uri, state, connection)?;
            }
        }
        DidCloseTextDocument::METHOD => {
            let params: DidCloseTextDocumentParams = serde_json::from_value(notif.params.clone())?;
            let uri = params.text_document.uri;

            state.documents.close(&uri);
            state.analyses.remove(uri.as_str());

            // Clear diagnostics.
            publish_diagnostics(connection, uri, Vec::new(), 0)?;
        }
        _ => {
            // Unknown notification, ignore.
        }
    }

    Ok(())
}

/// Collect diagnostics from an analysis result — the shared reporting
/// policy from `ambient-analysis`, rendered for LSP.
pub(crate) fn collect_diagnostics(
    doc: Option<&crate::documents::Document>,
    result: &ambient_analysis::AnalysisResult,
) -> Vec<Diagnostic> {
    let Some(doc) = doc else {
        return Vec::new();
    };

    result
        .diagnostics()
        .iter()
        .map(|d| diagnostic_to_lsp(doc, d))
        .collect()
}

/// Publish diagnostics to the client.
pub(crate) fn publish_diagnostics(
    connection: &Connection,
    uri: Uri,
    diagnostics: Vec<Diagnostic>,
    version: i32,
) -> anyhow::Result<()> {
    let params = PublishDiagnosticsParams {
        uri,
        diagnostics,
        version: Some(version),
    };

    let notification = Notification::new(
        PublishDiagnostics::METHOD.to_string(),
        serde_json::to_value(params)?,
    );

    connection
        .sender
        .send(Message::Notification(notification))?;
    Ok(())
}

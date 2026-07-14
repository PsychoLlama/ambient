//! LSP server implementation for the Ambient language.
//!
//! The server is a renderer over `ambient-analysis`: every diagnostic,
//! definition, and symbol comes from the same pipeline `ambient check`
//! runs, with the engine's `ModuleRegistry` as the single source of
//! cross-module truth. There is deliberately no LSP-private index of
//! modules or exports.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, PrepareRenameRequest,
    References, Rename, Request as _, SemanticTokensFullRequest, WorkspaceSymbolRequest,
};
use lsp_types::{
    CompletionOptions, CompletionParams, CompletionResponse, Diagnostic,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverContents, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    Location, MarkedString, MarkupContent, MarkupKind, OneOf, ReferenceParams, RenameOptions,
    SemanticTokens, SemanticTokensFullOptions, SemanticTokensOptions, SemanticTokensParams,
    SemanticTokensResult, SemanticTokensServerCapabilities, ServerCapabilities, SymbolInformation,
    SymbolKind as LspSymbolKind, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
    WorkDoneProgressOptions, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use serde_json::Value;

use ambient_analysis::occurrences::{Occurrence, SymbolTarget};
use ambient_analysis::package::AnalysisPackage;
use ambient_analysis::queries::{find_qname_module_at_offset, find_use_module_at_offset};
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;

use crate::analysis::{find_definition, find_expr_at_offset, find_item_at_offset};
use crate::completions::{CompletionContext, get_completions};
use crate::convert::{diagnostic_to_lsp, offset_range_to_lsp_range};
use crate::documents::DocumentStore;
use crate::hover_format::{format_expr_hover, format_item_hover, format_module_hover};
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
    /// Explicit override for the core-source cache base directory. The test
    /// harness sets a per-test temp dir here so materialization stays
    /// hermetic; `None` in production, where the platform cache dir is used.
    pub(crate) core_cache_base: Option<PathBuf>,
    /// The materialized core/platform source tree's root, once written.
    /// Builtin modules map to `file://` URIs under it (see [`module_uri`]), so
    /// goto-definition can point into embedded core/platform sources. Set once,
    /// lazily, on the first reanalysis (see [`ServerState::ensure_core_cache`]).
    pub(crate) core_cache_root: Option<PathBuf>,
}

impl ServerState {
    /// A fresh server: no open documents, no session, the platform-prelude
    /// ability resolver. The test harness builds state this way too, then
    /// optionally injects a pre-built in-memory [`AnalysisSession`].
    pub(crate) fn new() -> Self {
        Self {
            documents: DocumentStore::new(),
            analyses: HashMap::new(),
            session: None,
            occurrences: Vec::new(),
            ability_resolver: crate::analysis::platform_prelude_resolver(),
            core_cache_base: None,
            core_cache_root: None,
        }
    }

    /// Materialize the embedded core/platform sources once, recording the tree
    /// root so builtin modules gain navigable `file://` URIs. A no-op after the
    /// first call, and after any call that finds no cache location (navigation
    /// into builtins is then simply unavailable — never an error).
    pub(crate) fn ensure_core_cache(&mut self) {
        if self.core_cache_root.is_none() {
            self.core_cache_root =
                ambient_analysis::core_cache::materialize(self.core_cache_base.as_deref());
        }
    }

    /// Whether `uri` points at a file inside the materialized core-source tree.
    /// Such documents are read-only views: opening one publishes no diagnostics
    /// and triggers no analysis (its builtins are already checked in-place, and
    /// standalone analysis would flag their `unique(...)`/`extern fn` shapes).
    pub(crate) fn is_core_cache_uri(&self, uri: &Uri) -> bool {
        let (Some(root), Some(path)) = (self.core_cache_root.as_ref(), uri_to_path(uri)) else {
            return false;
        };
        path.starts_with(root)
    }

    /// The current package, if a package session is loaded.
    pub(crate) fn package(&self) -> Option<&AnalysisPackage> {
        self.session
            .as_ref()
            .map(ambient_analysis::session::AnalysisSession::package)
    }

    /// The current shared package registry, if a package session is loaded.
    pub(crate) fn registry(&self) -> Option<&Arc<ModuleRegistry>> {
        self.session
            .as_ref()
            .map(ambient_analysis::session::AnalysisSession::registry)
    }

    /// The in-memory source of the package module backing `uri`, if any — the
    /// exact text analysis ran against, not the file on disk.
    fn module_source_for_uri(&self, uri: &Uri) -> Option<String> {
        let session = self.session.as_ref()?;
        let module_path = session.package().module_path_for(&uri_to_path(uri)?)?;
        let module = session.package().modules.get(&module_path.to_string())?;
        Some(module.source.clone())
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

/// The main server loop — the *only* place that touches the connection. The
/// handlers are pure functions of [`ServerState`] that return what to send (a
/// [`Response`] or a batch of `DiagnosticsUpdate`s); this loop performs every
/// `send`. That seam lets the harness call the handlers directly, no channel.
fn main_loop(connection: &Connection) -> anyhow::Result<()> {
    let mut state = ServerState::new();

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
                // A malformed notification (e.g. `didChange` params that fail to
                // deserialize) must not kill the server: the renderer logs it and
                // keeps serving. A conformant client never sends one, but a bad
                // one is the client's bug, not grounds to drop the session.
                match handle_notification(&notif, &mut state) {
                    Ok(updates) => {
                        for update in updates {
                            connection
                                .sender
                                .send(crate::reanalyze::diagnostics_message(update)?)?;
                        }
                    }
                    Err(err) => {
                        eprintln!("ignoring malformed notification `{}`: {err}", notif.method);
                    }
                }
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
pub(crate) fn handle_request(req: &Request, state: &ServerState) -> Response {
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
            Ok(params) => crate::rename::handle_prepare_rename(id, &params, state),
            Err(e) => e,
        },
        Rename::METHOD => match parse_params(&req.params, &id) {
            Ok(params) => crate::rename::handle_rename(id, &params, state),
            Err(e) => e,
        },
        _ => Response::new_err(id, -32601, format!("Unknown method: {}", req.method)),
    }
}

/// The URI for a module: a package module's on-disk file, or a builtin
/// (core/platform) module's materialized file under `core_cache_root`.
///
/// This is the single module→URI seam. Package modules have real files;
/// core/platform modules are embedded in the binary, so they only navigate
/// once their sources have been materialized (see
/// [`ServerState::ensure_core_cache`]) — the URI then points at the on-disk
/// copy the editor will open. `None` when the module is neither.
pub(crate) fn module_uri(
    package: Option<&AnalysisPackage>,
    core_cache_root: Option<&Path>,
    module_path: &ModulePath,
) -> Option<Uri> {
    if let Some(package) = package
        && package.modules.contains_key(&module_path.to_string())
    {
        return path_to_uri(&package.file_for_module(module_path));
    }
    // A builtin module: point at its materialized source file, but only when it
    // actually exists on disk (guards against a bogus module path minting a URI
    // to a nonexistent file).
    let root = core_cache_root?;
    let file = ambient_analysis::core_cache::builtin_file(root, module_path);
    file.exists().then(|| path_to_uri(&file))?
}

/// Compute an LSP range in a possibly-unopened file: the open document when
/// available, else the session's in-memory source for that module (empty range
/// if the uri maps to neither).
pub(crate) fn range_in_file(
    state: &ServerState,
    uri: &Uri,
    start: usize,
    end: usize,
) -> lsp_types::Range {
    if let Some(doc) = state.documents.get(uri) {
        return offset_range_to_lsp_range(doc, start, end);
    }
    if let Some(content) = state.module_source_for_uri(uri) {
        let temp_doc = crate::documents::Document::new(uri.clone(), 0, content);
        return offset_range_to_lsp_range(&temp_doc, start, end);
    }
    // A materialized builtin file (or any unopened file:// target): read it off
    // disk to turn the definition's byte span into a line/character range. The
    // materialized copy is byte-identical to the source the span indexes.
    if let Some(path) = uri_to_path(uri)
        && let Ok(content) = std::fs::read_to_string(&path)
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

    // A method call/perform whose name is under the cursor: render the resolved
    // method's declaration signature, located through the occurrence index (the
    // same checked-AST-derived resolution goto/references use).
    if let Some((_, occ)) = occurrence_at(state, uri, offset)
        && let SymbolTarget::Method { .. } = &occ.target
        && let Some(content) = method_hover_content(state, registry, &occ.target)
    {
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: content,
            }),
            range: Some(offset_range_to_lsp_range(
                doc,
                occ.span.start as usize,
                occ.span.end as usize,
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
    let offset = offset as u32;

    let Some(definition) = find_definition(
        &analysis.result.module,
        &analysis.module_path,
        &analysis.registry,
        offset,
    ) else {
        // Not an item/local reference. A method call (`x.show()`, `Enum::default`,
        // a perform) is served from the occurrence index — the same
        // checked-AST-derived source `find-references` reads — by jumping to the
        // matching method declaration's occurrence. Core/platform methods have no
        // indexed URI, so those return null (out of scope for this phase).
        let location = occurrence_at(state, uri, offset).and_then(|(_, occ)| {
            matches!(occ.target, SymbolTarget::Method { .. })
                .then(|| method_definition_location(state, &occ.target))
                .flatten()
        });
        return match location {
            Some(location) => {
                let response = GotoDefinitionResponse::Scalar(location);
                Response::new_ok(id, serde_json::to_value(response).unwrap_or(Value::Null))
            }
            None => Response::new_ok(id, Value::Null),
        };
    };

    // A definition in another module needs a file to point at. Package modules
    // have one; core/platform modules navigate through their materialized
    // sources (or return null when materialization found no cache location).
    let target_uri = match &definition.module {
        Some(module_path) if *module_path != analysis.module_path => {
            match module_uri(
                state.package(),
                state.core_cache_root.as_deref(),
                module_path,
            ) {
                Some(target) => target,
                None => return Response::new_ok(id, Value::Null),
            }
        }
        _ => uri.clone(),
    };

    let range = range_in_file(
        state,
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
pub(crate) fn occurrence_at<'a>(
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
                state,
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

/// Hover markdown for a method `target`: locate its declaration occurrence
/// (module + name span) in the index, then render the declaration's signature
/// from that module's AST. `None` when the declaration isn't indexed.
fn method_hover_content(
    state: &ServerState,
    registry: &ModuleRegistry,
    target: &SymbolTarget,
) -> Option<String> {
    let (module_path, span) = state.occurrences.iter().find_map(|m| {
        m.occurrences
            .iter()
            .find(|o| o.is_definition && o.target == *target)
            .map(|o| (m.module_path.clone(), o.span))
    })?;
    crate::hover_format::format_method_hover(registry, &module_path, span)
}

/// The location of `target`'s definition occurrence, scanning the occurrence
/// index package-wide (a method's declaration may live in any module). `None`
/// when no indexed definition exists — e.g. a core/platform method, whose module
/// has no editor URI.
fn method_definition_location(state: &ServerState, target: &SymbolTarget) -> Option<Location> {
    for module in &state.occurrences {
        for occ in &module.occurrences {
            if occ.is_definition && occ.target == *target {
                let range = range_in_file(
                    state,
                    &module.uri,
                    occ.span.start as usize,
                    occ.span.end as usize,
                );
                return Some(Location {
                    uri: module.uri.clone(),
                    range,
                });
            }
        }
    }
    None
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

    let symbols = crate::document_symbols::extract_document_symbols(&analysis.result.module, doc);
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
            let range = range_in_file(state, &uri, sym.span.0 as usize, sym.span.1 as usize);

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

/// Handle an incoming notification, returning the diagnostics the transport
/// should publish (empty for notifications that produce none). Pure over
/// [`ServerState`]: it never touches the connection, so the test harness can
/// call it directly and read the returned diagnostics without a channel.
pub(crate) fn handle_notification(
    notif: &Notification,
    state: &mut ServerState,
) -> anyhow::Result<Vec<crate::reanalyze::DiagnosticsUpdate>> {
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
            // A harness that injected an in-memory session already has one, so
            // this on-disk discovery is skipped there.
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

            Ok(crate::reanalyze::reanalyze_all(&uri, state))
        }
        DidChangeTextDocument::METHOD => {
            let params: DidChangeTextDocumentParams = serde_json::from_value(notif.params.clone())?;
            let uri = params.text_document.uri.clone();
            let version = params.text_document.version;

            // We use full sync, so there's exactly one change with the full text.
            if let Some(change) = params.content_changes.into_iter().next() {
                state.documents.update(&uri, version, change.text);
                Ok(crate::reanalyze::reanalyze_all(&uri, state))
            } else {
                Ok(Vec::new())
            }
        }
        DidCloseTextDocument::METHOD => {
            let params: DidCloseTextDocumentParams = serde_json::from_value(notif.params.clone())?;
            let uri = params.text_document.uri;

            state.documents.close(&uri);
            state.analyses.remove(uri.as_str());

            // Clear diagnostics for the closed document.
            Ok(vec![crate::reanalyze::DiagnosticsUpdate {
                uri,
                diagnostics: Vec::new(),
                version: 0,
            }])
        }
        _ => {
            // Unknown notification, ignore.
            Ok(Vec::new())
        }
    }
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

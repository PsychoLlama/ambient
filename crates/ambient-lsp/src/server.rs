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

use ambient_analysis::occurrences::{Occurrence, SymbolTarget, collect_occurrences};
use ambient_analysis::package::AnalysisPackage;
use ambient_analysis::queries::{
    find_qname_module_at_offset, find_use_module_at_offset, resolve_qualified_name,
};
use ambient_engine::ast::{ItemKind, Module, QualifiedName};
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::{ExportKind, ModuleRegistry};

use crate::analysis::{
    find_definition, find_expr_at_offset, find_item_at_offset, format_type, format_type_hover,
};
use crate::completions::{CompletionContext, get_completions};
use crate::convert::{diagnostic_to_lsp, offset_range_to_lsp_range};
use crate::documents::DocumentStore;
use crate::semantic_tokens::{create_legend, extract_semantic_tokens};
use crate::util::{path_to_uri, uri_to_path};

/// The analysis of one open document, plus the context needed to resolve
/// names from it: its module path and the registry it was checked
/// against. Handlers never resolve through anything else.
struct DocumentAnalysis {
    result: ambient_analysis::AnalysisResult,
    module_path: ModulePath,
    registry: Arc<ModuleRegistry>,
}

/// One module's occurrence list plus where to render its spans.
///
/// References and rename both read this: the occurrence list is the source of
/// exact reference ranges, and `uri` turns a module-local span into an LSP
/// [`Location`]. Rebuilt from the package's parsed modules on every edit, so
/// results are always fresh.
struct ModuleOccurrences {
    module_path: ModulePath,
    uri: Uri,
    occurrences: Vec<Occurrence>,
}

/// Server-wide state.
struct ServerState {
    documents: DocumentStore,
    /// Per-document analyses, keyed by URI string (Uri has interior
    /// mutability, so it can't be a map key).
    analyses: HashMap<String, DocumentAnalysis>,
    /// The package containing the open documents, if any.
    package: Option<AnalysisPackage>,
    /// The registry for the whole package (shared by all open documents
    /// when a package is loaded). Rebuilt on every edit.
    package_registry: Option<Arc<ModuleRegistry>>,
    /// Occurrence index backing find-references and rename: every definition
    /// and reference site of every symbol, with exact spans, for every module
    /// in the package (opened or not). Rebuilt on every edit alongside the
    /// registry, so it never goes stale.
    occurrences: Vec<ModuleOccurrences>,
    /// Ability resolver for completions/hover: the platform prelude plus
    /// builtins, the same interfaces analysis checks against.
    ability_resolver: ambient_engine::ability_resolver::AbilityResolver,
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
        package: None,
        package_registry: None,
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
fn module_uri(package: Option<&AnalysisPackage>, module_path: &ModulePath) -> Option<Uri> {
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

/// Format hover content for an item definition, including documentation.
fn format_item_hover(item: &ambient_engine::ast::Item) -> String {
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
fn format_item_signature(kind: &ItemKind, content: &mut String) {
    match kind {
        ItemKind::Function(f) => format_function_hover(f, content),
        ItemKind::Const(c) => {
            content.push_str("const ");
            content.push_str(&c.name);
            content.push_str(": ");
            content.push_str(&format_type(&c.ty));
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
                content.push_str(&trait_name.name);
                content.push_str(" for ");
            }
            content.push_str(&format_type(&i.for_type));
        }
    }
}

/// Format a function's signature for hover.
fn format_function_hover(f: &ambient_engine::ast::FunctionDef, content: &mut String) {
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
fn format_type_params(type_params: &[ambient_engine::ast::TypeParam], content: &mut String) {
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

/// Format hover content for a module, reading path and docs from the
/// registry — the same module info the checker resolves imports against.
fn format_module_hover(module_path: &ModulePath, registry: &ModuleRegistry) -> String {
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
/// (`core::primitives::String`) rather than the bare `String`. The literal arms fall back
/// to that same FQN when inference hasn't attached a type, since a literal's
/// primitive is unambiguous.
fn format_expr_hover(expr: &ambient_engine::ast::Expr) -> String {
    use ambient_engine::types::Primitive;
    match &expr.kind {
        ambient_engine::ast::ExprKind::Local(local_id) => {
            let type_info = expr
                .ty
                .as_ref()
                .map_or("unknown".to_string(), format_type_hover);
            format!("```ambient\nlocal_{local_id}: {type_info}\n```")
        }
        ambient_engine::ast::ExprKind::Name(qname) => {
            let type_info = expr
                .ty
                .as_ref()
                .map_or_else(|| "unknown".to_string(), format_type_hover);
            format!("```ambient\n{}: {type_info}\n```", qname.name)
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
            match module_uri(state.package.as_ref(), module_path) {
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
/// when it is a function or const defined in a package module — the symbols
/// whose references the occurrence index captures completely. Types, enums,
/// traits, abilities, and anything from core/platform are rejected: their
/// references (type positions, variant constructors) aren't indexed yet, so a
/// partial rename would break the code.
fn is_renameable_target(state: &ServerState, target: &SymbolTarget) -> bool {
    match target {
        SymbolTarget::Local { name, .. } => name.as_ref() != "self",
        SymbolTarget::Item { module, name, .. } => {
            let Some(package) = state.package.as_ref() else {
                return false;
            };
            if !package.modules.contains_key(&module.to_string()) {
                return false;
            }
            let Some(registry) = state.package_registry.as_ref() else {
                return false;
            };
            matches!(
                registry
                    .get(module)
                    .and_then(|info| info.exports.get(name.as_ref()))
                    .map(|export| export.kind),
                Some(ExportKind::Function | ExportKind::Const)
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
    let (Some(package), Some(registry)) = (state.package.as_ref(), state.package_registry.as_ref())
    else {
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
/// Symbols come from the registry's export tables (the same ones import
/// resolution reads), filtered to the package's own modules.
fn handle_workspace_symbol(
    id: RequestId,
    params: &WorkspaceSymbolParams,
    state: &ServerState,
) -> Response {
    let query = params.query.to_lowercase();
    let mut symbols: Vec<SymbolInformation> = Vec::new();

    if let (Some(package), Some(registry)) =
        (state.package.as_ref(), state.package_registry.as_ref())
    {
        let mut module_keys: Vec<_> = package.modules.keys().collect();
        module_keys.sort();

        for key in module_keys {
            let module = &package.modules[key];
            let Some(info) = registry.get(&module.path) else {
                continue;
            };
            let Some(uri) = module_uri(Some(package), &module.path) else {
                continue;
            };

            let mut exports: Vec<_> = info.exports.values().collect();
            exports.sort_by_key(|e| e.name_span.start);

            for export in exports {
                // Filter by query (case-insensitive substring match)
                if !query.is_empty() && !export.name.to_lowercase().contains(&query) {
                    continue;
                }

                let range = range_in_file(
                    &state.documents,
                    &uri,
                    export.name_span.start as usize,
                    export.name_span.end as usize,
                );

                #[allow(deprecated)] // SymbolInformation::deprecated field is deprecated
                symbols.push(SymbolInformation {
                    name: export.name.to_string(),
                    kind: export_kind_to_lsp(export.kind),
                    tags: None,
                    deprecated: None,
                    location: Location {
                        uri: uri.clone(),
                        range,
                    },
                    container_name: Some(module.path.to_string()),
                });
            }
        }
    }

    let response = WorkspaceSymbolResponse::Flat(symbols);
    Response::new_ok(id, serde_json::to_value(response).unwrap_or(Value::Null))
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
            Some(format_type(&c.ty)),
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
    }
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

/// Convert the engine's `ExportKind` to LSP `SymbolKind`.
fn export_kind_to_lsp(kind: ExportKind) -> LspSymbolKind {
    match kind {
        ExportKind::Function => LspSymbolKind::FUNCTION,
        ExportKind::Const => LspSymbolKind::CONSTANT,
        ExportKind::Struct => LspSymbolKind::STRUCT,
        ExportKind::TypeAlias => LspSymbolKind::TYPE_PARAMETER,
        ExportKind::Enum => LspSymbolKind::ENUM,
        ExportKind::EnumVariant => LspSymbolKind::ENUM_MEMBER,
        ExportKind::Ability | ExportKind::Trait => LspSymbolKind::INTERFACE,
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
        .map(|(n, t)| format!("{n}: {}", format_type(t)))
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
            if state.package.is_none()
                && let Some(file_path) = uri_to_path(&uri)
                && let Some(mut package) = AnalysisPackage::discover(&file_path)
            {
                package.load_modules();
                state.package = Some(package);
            }

            reanalyze_all(&uri, state, connection)?;
        }
        DidChangeTextDocument::METHOD => {
            let params: DidChangeTextDocumentParams = serde_json::from_value(notif.params.clone())?;
            let uri = params.text_document.uri.clone();
            let version = params.text_document.version;

            // We use full sync, so there's exactly one change with the full text.
            if let Some(change) = params.content_changes.into_iter().next() {
                state.documents.update(&uri, version, change.text);
                reanalyze_all(&uri, state, connection)?;
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
fn collect_diagnostics(
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
fn publish_diagnostics(
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

/// Re-analyze after a document change: update the package's view of the
/// changed file, rebuild the registry once, then re-analyze every open
/// document against it. A signature change in one file must surface (or
/// clear) type errors in files that import it.
fn reanalyze_all(
    changed_uri: &Uri,
    state: &mut ServerState,
    connection: &Connection,
) -> anyhow::Result<()> {
    // Update the package's copy of the changed document.
    if let Some(package) = state.package.as_mut()
        && let Some(file_path) = uri_to_path(changed_uri)
        && let Some(module_path) = package.module_path_for(&file_path)
        && let Some(doc) = state.documents.get(changed_uri)
    {
        package.insert_module(module_path, doc.text.clone());
    }

    // Rebuild the shared registry once per change.
    state.package_registry = state
        .package
        .as_ref()
        .map(|package| Arc::new(package.build_registry()));

    let uris: Vec<Uri> = state.documents.uris().cloned().collect();
    for uri in uris {
        reanalyze_document(&uri, state, connection)?;
    }

    // Refresh the occurrence index against the rebuilt registry, so
    // find-references and rename never see stale results.
    rebuild_occurrence_index(state);
    Ok(())
}

/// Rebuild the whole-package occurrence index from the current parsed modules
/// and registry.
///
/// Walks every package module (opened or not) so references and rename reach
/// files never opened in the editor. With no package, each open document is
/// indexed as a standalone root. Cheap enough to run on every edit — a pure
/// AST walk that resolves names through the already-built registry.
fn rebuild_occurrence_index(state: &mut ServerState) {
    let mut index = Vec::new();

    if let (Some(package), Some(registry)) =
        (state.package.as_ref(), state.package_registry.as_ref())
    {
        for module in package.modules.values() {
            let Some(uri) = module_uri(Some(package), &module.path) else {
                continue;
            };
            let occurrences = collect_occurrences(&module.ast, &module.path, registry);
            index.push(ModuleOccurrences {
                module_path: module.path.clone(),
                uri,
                occurrences,
            });
        }
    } else {
        for (uri_str, analysis) in &state.analyses {
            let Ok(uri) = uri_str.parse::<Uri>() else {
                continue;
            };
            let occurrences = collect_occurrences(
                &analysis.result.module,
                &analysis.module_path,
                &analysis.registry,
            );
            index.push(ModuleOccurrences {
                module_path: analysis.module_path.clone(),
                uri,
                occurrences,
            });
        }
    }

    state.occurrences = index;
}

/// Re-analyze one open document against the current registry and publish
/// fresh diagnostics.
fn reanalyze_document(
    uri: &Uri,
    state: &mut ServerState,
    connection: &Connection,
) -> anyhow::Result<()> {
    let Some(doc) = state.documents.get(uri) else {
        return Ok(());
    };

    let package_context = state.package.as_ref().zip(state.package_registry.as_ref());
    let (module_path, registry) = if let Some((package, registry)) = package_context
        && let Some(file_path) = uri_to_path(uri)
        && let Some(module_path) = package.module_path_for(&file_path)
    {
        (module_path, Arc::clone(registry))
    } else {
        // No package: the document checks as a stand-alone package root
        // against the core+platform registry, same as `ambient check` on
        // a bare file.
        let module_path = ModulePath::root();
        let mut registry = ambient_analysis::core_platform_registry();
        let recovered = ambient_parser::parse_recovering(&doc.text);
        registry.register(&module_path, Arc::new(recovered.module));
        (module_path, Arc::new(registry))
    };

    let result =
        ambient_analysis::analyze_with_registry(&doc.text, Some(&module_path), Some(&registry));

    let diagnostics = collect_diagnostics(Some(doc), &result);
    publish_diagnostics(connection, uri.clone(), diagnostics, doc.version)?;

    state.analyses.insert(
        uri.as_str().to_string(),
        DocumentAnalysis {
            result,
            module_path,
            registry,
        },
    );
    Ok(())
}

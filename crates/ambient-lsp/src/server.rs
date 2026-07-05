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
use lsp_types::notification::Progress;
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, References, Request as _,
    SemanticTokensFullRequest, WorkspaceSymbolRequest,
};
use lsp_types::{
    CompletionOptions, CompletionParams, CompletionResponse, Diagnostic,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, Location, MarkedString, MarkupContent, MarkupKind,
    NumberOrString, OneOf, ProgressParams, ProgressParamsValue, PublishDiagnosticsParams,
    ReferenceParams, SemanticTokens, SemanticTokensFullOptions, SemanticTokensOptions,
    SemanticTokensParams, SemanticTokensResult, SemanticTokensServerCapabilities,
    ServerCapabilities, SymbolInformation, SymbolKind as LspSymbolKind, TextDocumentSyncCapability,
    TextDocumentSyncKind, Uri, WorkDoneProgress, WorkDoneProgressBegin, WorkDoneProgressEnd,
    WorkDoneProgressReport, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use serde_json::Value;

use ambient_analysis::package::AnalysisPackage;
use ambient_analysis::queries::{
    find_qname_module_at_offset, find_use_module_at_offset, resolve_qualified_name,
};
use ambient_engine::ast::{ItemKind, Module};
use ambient_engine::build::{ParseFailure, build_package};
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::{ExportKind, ModuleRegistry};
use ambient_engine::symbol_db::SymbolDb;

use crate::analysis::{find_definition, find_expr_at_offset, find_item_at_offset, format_type};
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
    /// Symbol database for find-references (populated by a full package
    /// compile at first open; see `populate_symbol_db_from_package`).
    symbol_db: Option<SymbolDb>,
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
        symbol_db: None,
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
fn format_expr_hover(expr: &ambient_engine::ast::Expr) -> String {
    match &expr.kind {
        ambient_engine::ast::ExprKind::Local(local_id) => {
            let type_info = expr.ty.as_ref().map_or("unknown".to_string(), format_type);
            format!("```ambient\nlocal_{local_id}: {type_info}\n```")
        }
        ambient_engine::ast::ExprKind::Name(qname) => {
            let type_info = expr
                .ty
                .as_ref()
                .map_or_else(|| "unknown".to_string(), format_type);
            format!("```ambient\n{}: {type_info}\n```", qname.name)
        }
        ambient_engine::ast::ExprKind::Bool(b) => {
            let type_info = expr.ty.as_ref().map_or("bool".to_string(), format_type);
            format!("```ambient\n{b}: {type_info}\n```")
        }
        ambient_engine::ast::ExprKind::Number(n) => {
            let type_info = expr.ty.as_ref().map_or("number".to_string(), format_type);
            format!("```ambient\n{n}: {type_info}\n```")
        }
        ambient_engine::ast::ExprKind::String(s) => {
            let type_info = expr.ty.as_ref().map_or("string".to_string(), format_type);
            format!("```ambient\n\"{s}\": {type_info}\n```")
        }
        ambient_engine::ast::ExprKind::RecordField(_, field_name) => {
            let type_info = expr.ty.as_ref().map_or("unknown".to_string(), format_type);
            format!("```ambient\n{field_name}: {type_info}\n```")
        }
        _ => {
            let type_info = expr.ty.as_ref().map_or("unknown".to_string(), format_type);
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

/// Handle find references request.
fn handle_references(id: RequestId, params: &ReferenceParams, state: &ServerState) -> Response {
    // Helper for returning empty references list
    let empty_response = || Response::new_ok(id.clone(), Value::Array(vec![]));

    let uri = &params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;

    let Some(doc) = state.documents.get(uri) else {
        return empty_response();
    };

    let Some(analysis) = state.analyses.get(uri.as_str()) else {
        return empty_response();
    };
    let module = &analysis.result.module;

    let offset = doc.position_to_offset(position.line, position.character);

    // Find the expression at the cursor position
    #[allow(clippy::cast_possible_truncation)]
    let Some(expr) = find_expr_at_offset(module, offset as u32) else {
        return empty_response();
    };

    // Extract the qualified name from the expression
    let qname = match &expr.kind {
        ambient_engine::ast::ExprKind::Name(qname) => qname,
        ambient_engine::ast::ExprKind::Call(callee, _) => {
            if let ambient_engine::ast::ExprKind::Name(qname) = &callee.kind {
                qname
            } else {
                return empty_response();
            }
        }
        _ => {
            return empty_response();
        }
    };

    // Resolve to the defining module through the registry.
    let defining_module =
        resolve_qualified_name(module, &analysis.module_path, &analysis.registry, qname)
            .and_then(|d| d.module)
            .unwrap_or_else(|| analysis.module_path.clone());

    // Get the symbol's hash from the database
    let Some(db) = state.symbol_db.as_ref() else {
        return empty_response();
    };

    let target_module_path = defining_module.to_string();
    let Ok(symbols) = db.search_symbols(&qname.name) else {
        return empty_response();
    };

    // Find the matching symbol by module path. The root module records
    // an empty module path in the database.
    let target_entry = symbols.iter().find(|entry| {
        entry.module_path == target_module_path
            || (entry.module_path.is_empty() && defining_module == ModulePath::root())
    });

    let Some(target_entry) = target_entry else {
        return empty_response();
    };

    // Query all dependents (functions that call this one)
    let Ok(dependent_hashes) = db.get_dependents(target_entry.hash) else {
        return empty_response();
    };

    // Resolve each dependent to a location
    let mut locations = Vec::new();

    // Optionally include the definition itself
    if params.context.include_declaration
        && let Some(loc) = resolve_symbol_to_location(&target_entry.path, state)
    {
        locations.push(loc);
    }

    // Add all reference locations
    for dep_hash in dependent_hashes {
        let Ok(dep_paths) = db.get_symbol_paths(dep_hash) else {
            continue;
        };
        for path in dep_paths {
            if let Some(loc) = resolve_symbol_to_location(&path, state) {
                locations.push(loc);
            }
        }
    }

    Response::new_ok(id, serde_json::to_value(locations).unwrap_or(Value::Null))
}

/// Resolve a symbol path (e.g., "pkg.module.name") to an LSP Location,
/// using the package registry's span-carrying exports.
fn resolve_symbol_to_location(symbol_path: &str, state: &ServerState) -> Option<Location> {
    let registry = state.package_registry.as_ref()?;

    // Parse symbol path: "package.module1.module2.name".
    let parts: Vec<&str> = symbol_path.split('.').collect();
    let (name, module_segments) = match parts.as_slice() {
        [] | [_] => return None,
        [_package, name] => (*name, Vec::new()),
        [_package, segments @ .., name] => (*name, segments.to_vec()),
    };

    let module_path = if module_segments.is_empty() {
        ModulePath::root()
    } else {
        ModulePath::from_str_segments(&module_segments)?
    };

    let info = registry.get(&module_path)?;
    let export = info.exports.get(name)?;

    let uri = module_uri(state.package.as_ref(), &module_path)?;
    let range = range_in_file(
        &state.documents,
        &uri,
        export.name_span.start as usize,
        export.name_span.end as usize,
    );

    Some(Location { uri, range })
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

/// Parse source code into an AST (wrapper for `ambient_parser::parse`).
fn parse_source(source: &str) -> Result<Module, ParseFailure> {
    ambient_parser::parse(source).map_err(|e| ParseFailure {
        message: e.kind.to_string(),
        span: (e.span.start, e.span.end),
        context: e.context,
    })
}

/// Populate the symbol database by compiling the package.
///
/// This compiles all modules and populates the symbol database with
/// function definitions and their dependencies.
fn populate_symbol_db_from_package(
    db: &mut SymbolDb,
    package: &AnalysisPackage,
    connection: &Connection,
) {
    let token = NumberOrString::String("indexing".to_string());

    // Send progress begin
    let begin = ProgressParams {
        token: token.clone(),
        value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(WorkDoneProgressBegin {
            title: "Indexing".to_string(),
            cancellable: Some(false),
            message: Some("Compiling package...".to_string()),
            percentage: Some(0),
        })),
    };
    let _ = send_progress(connection, begin);

    // Build the package with progress callback
    #[allow(clippy::cast_possible_truncation)]
    let progress_cb = |module: &str, current: usize, total: usize| {
        let percentage = (current * 100).checked_div(total).map(|p| p as u32);

        let report = ProgressParams {
            token: token.clone(),
            value: ProgressParamsValue::WorkDone(WorkDoneProgress::Report(
                WorkDoneProgressReport {
                    cancellable: Some(false),
                    message: Some(format!("[{current}/{total}] {module}")),
                    percentage,
                },
            )),
        };
        let _ = send_progress(connection, report);
    };

    let result = build_package(
        &package.root,
        parse_source,
        ambient_platform::ABILITY_DECLARATIONS,
        Some(&progress_cb),
    );

    // Send progress end
    let end_message = match &result {
        Ok(r) => Some(format!("Indexed {} modules", r.module_count)),
        Err(e) => Some(format!("Indexing failed: {e}")),
    };

    let end = ProgressParams {
        token,
        value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(WorkDoneProgressEnd {
            message: end_message,
        })),
    };
    let _ = send_progress(connection, end);

    // Log the result (the database was populated by build_package)
    match result {
        Ok(r) => {
            log::info!(
                "Indexed {} modules for package {}",
                r.module_count,
                r.package_name
            );
            // The build_package function already populates the symbol database
            // We need to reload it since build_package creates its own instance
            let db_path = package.root.join("build").join("symbols.db");
            if let Ok(new_db) = SymbolDb::open(&db_path) {
                *db = new_db;
            }
        }
        Err(e) => {
            log::warn!("Failed to index package: {e}");
        }
    }
}

/// Send a progress notification to the client.
fn send_progress(connection: &Connection, params: ProgressParams) -> anyhow::Result<()> {
    let notif = Notification::new(Progress::METHOD.to_string(), params);
    connection.sender.send(Message::Notification(notif))?;
    Ok(())
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

            // Discover package if not already discovered
            if state.package.is_none()
                && let Some(file_path) = uri_to_path(&uri)
                && let Some(mut package) = AnalysisPackage::discover(&file_path)
            {
                package.load_modules();

                // Initialize the symbol database
                let db_path = package.root.join("build").join("symbols.db");
                if let Some(parent) = db_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Ok(mut db) = SymbolDb::open(&db_path) {
                    // Compile package and populate database
                    populate_symbol_db_from_package(&mut db, &package, connection);
                    state.symbol_db = Some(db);
                }

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
///
/// Note the symbol database is *not* refreshed here — it is populated
/// once from a full package compile at first open. Find-references can
/// go stale after edits; making it incrementally updatable is an
/// engine-level gap.
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
    Ok(())
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

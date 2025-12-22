//! LSP server implementation for the Ambient language.

use std::collections::HashMap;
use std::str::FromStr;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, Request as _,
    SemanticTokensFullRequest, WorkspaceSymbolRequest,
};
use lsp_types::{
    CompletionOptions, CompletionParams, CompletionResponse, Diagnostic,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, Location, MarkedString, MarkupContent, MarkupKind, OneOf,
    PublishDiagnosticsParams, SemanticTokens, SemanticTokensFullOptions, SemanticTokensOptions,
    SemanticTokensParams, SemanticTokensResult, SemanticTokensServerCapabilities,
    ServerCapabilities, SymbolInformation, SymbolKind as LspSymbolKind, TextDocumentSyncCapability,
    TextDocumentSyncKind, Uri, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use serde_json::Value;

use ambient_engine::ast::{ItemKind, Module};
use ambient_engine::symbol_db::{
    compute_export_hash, compute_source_hash, extract_dependencies, extract_symbols, ModuleInfo,
    SymbolDb,
};

use crate::analysis::{
    analyze_with_registry, find_definition_cross_file, find_expr_at_offset, find_item_at_offset,
    format_type,
};
use crate::completions::{get_completions, CompletionContext};
use crate::convert::{
    offset_range_to_lsp_range, parse_error_to_diagnostic, type_error_to_diagnostic,
};
use crate::documents::DocumentStore;
use crate::package::PackageInfo;
use crate::semantic_tokens::{create_legend, extract_semantic_tokens};
use crate::util::uri_to_path;
use crate::workspace::SymbolKind;
use crate::workspace::WorkspaceIndex;

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
    let mut documents = DocumentStore::new();
    // Use String keys instead of Uri to avoid mutable_key_type warning (Uri has interior mutability).
    let mut analysis_cache: HashMap<String, crate::analysis::AnalysisResult> = HashMap::new();
    // Workspace index for cross-file navigation
    let mut workspace_index = WorkspaceIndex::new();
    // Package info for cross-module type checking
    let mut package_info: Option<PackageInfo> = None;
    // Symbol database for fast lookups (initialized when package is discovered)
    let mut symbol_db: Option<SymbolDb> = None;

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }

                let response = handle_request(
                    &req,
                    &documents,
                    &analysis_cache,
                    &workspace_index,
                    symbol_db.as_ref(),
                );
                connection.sender.send(Message::Response(response))?;
            }
            Message::Notification(notif) => {
                handle_notification(
                    &notif,
                    &mut documents,
                    &mut analysis_cache,
                    &mut workspace_index,
                    &mut package_info,
                    &mut symbol_db,
                    connection,
                )?;
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
fn handle_request(
    req: &Request,
    documents: &DocumentStore,
    analysis_cache: &HashMap<String, crate::analysis::AnalysisResult>,
    workspace_index: &WorkspaceIndex,
    symbol_db: Option<&SymbolDb>,
) -> Response {
    let id = req.id.clone();

    match req.method.as_str() {
        HoverRequest::METHOD => {
            let params = match parse_params(&req.params, &id) {
                Ok(p) => p,
                Err(e) => return e,
            };
            handle_hover(
                id,
                &params,
                documents,
                analysis_cache,
                workspace_index,
                symbol_db,
            )
        }
        GotoDefinition::METHOD => {
            let params = match parse_params(&req.params, &id) {
                Ok(p) => p,
                Err(e) => return e,
            };
            handle_goto_definition(
                id,
                &params,
                documents,
                analysis_cache,
                workspace_index,
                symbol_db,
            )
        }
        Completion::METHOD => {
            let params = match parse_params(&req.params, &id) {
                Ok(p) => p,
                Err(e) => return e,
            };
            handle_completion(id, &params, documents, analysis_cache, symbol_db)
        }
        DocumentSymbolRequest::METHOD => {
            let params = match parse_params(&req.params, &id) {
                Ok(p) => p,
                Err(e) => return e,
            };
            handle_document_symbol(id, &params, documents, analysis_cache)
        }
        WorkspaceSymbolRequest::METHOD => {
            let params = match parse_params(&req.params, &id) {
                Ok(p) => p,
                Err(e) => return e,
            };
            handle_workspace_symbol(id, &params, workspace_index, documents, symbol_db)
        }
        SemanticTokensFullRequest::METHOD => {
            let params = match parse_params(&req.params, &id) {
                Ok(p) => p,
                Err(e) => return e,
            };
            handle_semantic_tokens(id, &params, documents, analysis_cache)
        }
        _ => Response::new_err(id, -32601, format!("Unknown method: {}", req.method)),
    }
}

/// Handle hover request.
fn handle_hover(
    id: RequestId,
    params: &HoverParams,
    documents: &DocumentStore,
    analysis_cache: &HashMap<String, crate::analysis::AnalysisResult>,
    workspace_index: &WorkspaceIndex,
    symbol_db: Option<&SymbolDb>,
) -> Response {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;

    let Some(doc) = documents.get(uri) else {
        return Response::new_ok(id, Value::Null);
    };

    let uri_str = uri.as_str();
    let Some(analysis) = analysis_cache.get(uri_str) else {
        return Response::new_ok(id, Value::Null);
    };

    let Some(module) = &analysis.module else {
        return Response::new_ok(id, Value::Null);
    };

    let offset = doc.position_to_offset(position.line, position.character);

    #[allow(clippy::cast_possible_truncation)]
    let offset = offset as u32;

    // First, check if hovering over a module path in a use statement.
    if let Some(module_info) = workspace_index.find_use_module_at_offset(uri, offset) {
        let content = format_module_hover(module_info);
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: content,
            }),
            range: None, // TODO: compute range from path segment span
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
    if let ambient_engine::ast::ExprKind::Name(qname) = &expr.kind {
        if let Some(module_info) = find_qname_module_at_offset(qname, offset, uri, workspace_index)
        {
            let content = format_module_hover(module_info);
            let hover = Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: content,
                }),
                range: None,
            };
            return Response::new_ok(id, serde_json::to_value(hover).unwrap_or(Value::Null));
        }
    }

    // Build hover content based on expression kind.
    let content = format_expr_hover(expr, symbol_db);

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

    // Add type signature in code block.
    content.push_str("```ambient\n");
    match &item.kind {
        ItemKind::Function(f) => {
            if f.is_public {
                content.push_str("pub ");
            }
            content.push_str("fn ");
            content.push_str(&f.name);

            // Type parameters.
            if !f.type_params.is_empty() {
                content.push('<');
                for (i, tp) in f.type_params.iter().enumerate() {
                    if i > 0 {
                        content.push_str(", ");
                    }
                    content.push_str(&tp.name);
                }
                content.push('>');
            }

            // Parameters.
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

            // Return type.
            if let Some(ret) = &f.ret_ty {
                content.push_str(": ");
                content.push_str(&format_type(ret));
            }

            // Abilities.
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
        ItemKind::Const(c) => {
            content.push_str("const ");
            content.push_str(&c.name);
            content.push_str(": ");
            content.push_str(&format_type(&c.ty));
        }
        ItemKind::TypeAlias(t) => {
            content.push_str("type ");
            content.push_str(&t.name);
            if !t.type_params.is_empty() {
                content.push('<');
                for (i, tp) in t.type_params.iter().enumerate() {
                    if i > 0 {
                        content.push_str(", ");
                    }
                    content.push_str(&tp.name);
                }
                content.push('>');
            }
            content.push_str(" = ");
            content.push_str(&format_type(&t.ty));
        }
        ItemKind::Enum(e) => {
            content.push_str("enum ");
            content.push_str(&e.name);
            if !e.type_params.is_empty() {
                content.push('<');
                for (i, tp) in e.type_params.iter().enumerate() {
                    if i > 0 {
                        content.push_str(", ");
                    }
                    content.push_str(&tp.name);
                }
                content.push('>');
            }
        }
        ItemKind::Ability(a) => {
            content.push_str("ability ");
            content.push_str(&a.name);
        }
        ItemKind::Use(_) => {
            content.push_str("use ...");
        }
    }
    content.push_str("\n```");

    // Add documentation if present.
    if let Some(doc) = &item.doc {
        content.push_str("\n\n---\n\n");
        content.push_str(doc);
    }

    content
}

/// Format hover content for a module.
fn format_module_hover(module_info: &crate::workspace::ModuleInfo) -> String {
    let mut content = String::new();

    // Show module path
    content.push_str("```ambient\n");
    content.push_str("module ");
    content.push_str(&module_info.module_path.join("."));
    content.push_str("\n```");

    // Add documentation if present
    if let Some(doc) = &module_info.doc {
        content.push_str("\n\n---\n\n");
        content.push_str(doc);
    }

    content
}

/// Find the module referenced at a cursor position in a qualified name's path.
fn find_qname_module_at_offset<'a>(
    qname: &ambient_engine::ast::QualifiedName,
    offset: u32,
    _current_uri: &Uri,
    workspace_index: &'a WorkspaceIndex,
) -> Option<&'a crate::workspace::ModuleInfo> {
    // Check if we have path spans and if cursor is within any of them
    if qname.path_spans.len() != qname.path.len() {
        return None; // No span information available
    }

    for (idx, span) in qname.path_spans.iter().enumerate() {
        if offset >= span.start && offset < span.end {
            // Cursor is within this path segment - resolve the partial path
            let partial_path: Vec<_> = qname.path[..=idx].to_vec();

            // Try to resolve to a module
            // The path in a qualified name is relative to pkg root
            return workspace_index.find_module(&partial_path);
        }
    }

    None
}

/// Format hover content for an expression.
fn format_expr_hover(expr: &ambient_engine::ast::Expr, symbol_db: Option<&SymbolDb>) -> String {
    match &expr.kind {
        ambient_engine::ast::ExprKind::Local(local_id) => {
            let type_info = expr.ty.as_ref().map_or("unknown".to_string(), format_type);
            format!("```ambient\nlocal_{local_id}: {type_info}\n```")
        }
        ambient_engine::ast::ExprKind::Name(qname) => {
            // Try to look up type and documentation from SymbolDb for qualified names
            let db_info = lookup_qname_info(qname, symbol_db);

            // Use SymbolDb type if available, otherwise fall back to expression type
            let type_info = db_info
                .as_ref()
                .map(|info| format_type(&info.type_signature))
                .or_else(|| expr.ty.as_ref().map(format_type))
                .unwrap_or_else(|| "unknown".to_string());

            let doc = db_info.and_then(|info| info.doc);

            if let Some(doc) = doc {
                format!("```ambient\n{}: {type_info}\n```\n\n{doc}", qname.name)
            } else {
                format!("```ambient\n{}: {type_info}\n```", qname.name)
            }
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
    documents: &DocumentStore,
    analysis_cache: &HashMap<String, crate::analysis::AnalysisResult>,
    workspace_index: &WorkspaceIndex,
    symbol_db: Option<&SymbolDb>,
) -> Response {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;

    let Some(doc) = documents.get(uri) else {
        return Response::new_ok(id, Value::Null);
    };

    let uri_str = uri.as_str();
    let Some(analysis) = analysis_cache.get(uri_str) else {
        return Response::new_ok(id, Value::Null);
    };

    let Some(module) = &analysis.module else {
        return Response::new_ok(id, Value::Null);
    };

    let offset = doc.position_to_offset(position.line, position.character);

    #[allow(clippy::cast_possible_truncation)]
    let Some(def_result) =
        find_definition_cross_file(module, offset as u32, uri, workspace_index, symbol_db)
    else {
        return Response::new_ok(id, Value::Null);
    };

    // Determine target URI and document
    let (target_uri, target_doc) = if let Some(ref def_uri) = def_result.uri {
        // Cross-file definition - try to get the target document
        if let Some(target_doc) = documents.get(def_uri) {
            (def_uri.clone(), Some(target_doc))
        } else {
            // Document not open, still return location with zero range for now
            (def_uri.clone(), None)
        }
    } else {
        // Local definition
        (uri.clone(), Some(doc))
    };

    let range = if let Some(target_doc) = target_doc {
        offset_range_to_lsp_range(
            target_doc,
            def_result.span.start as usize,
            def_result.span.end as usize,
        )
    } else {
        // Document not open - try to read the file to compute proper range
        if let Some(file_path) = uri_to_path(&target_uri) {
            if let Ok(content) = std::fs::read_to_string(&file_path) {
                let temp_doc = crate::documents::Document::new(target_uri.clone(), 0, content);
                offset_range_to_lsp_range(
                    &temp_doc,
                    def_result.span.start as usize,
                    def_result.span.end as usize,
                )
            } else {
                // Can't read file, fall back to zero range
                lsp_types::Range::default()
            }
        } else {
            lsp_types::Range::default()
        }
    };

    let location = Location {
        uri: target_uri,
        range,
    };

    let response = GotoDefinitionResponse::Scalar(location);
    Response::new_ok(id, serde_json::to_value(response).unwrap_or(Value::Null))
}

/// Handle completion request.
fn handle_completion(
    id: RequestId,
    params: &CompletionParams,
    documents: &DocumentStore,
    analysis_cache: &HashMap<String, crate::analysis::AnalysisResult>,
    symbol_db: Option<&SymbolDb>,
) -> Response {
    let uri = &params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;

    let Some(doc) = documents.get(uri) else {
        return Response::new_ok(id, Value::Null);
    };

    let offset = doc.position_to_offset(position.line, position.character);

    // Get the module from the analysis cache (if available).
    let uri_str = uri.as_str();
    let module = analysis_cache
        .get(uri_str)
        .and_then(|analysis| analysis.module.as_ref());

    // Create completion context and get completions.
    let ctx = CompletionContext::new(&doc.text, offset);
    let items = get_completions(&ctx, module, symbol_db);

    let response = CompletionResponse::Array(items);
    Response::new_ok(id, serde_json::to_value(response).unwrap_or(Value::Null))
}

/// Handle document symbol request.
fn handle_document_symbol(
    id: RequestId,
    params: &DocumentSymbolParams,
    documents: &DocumentStore,
    analysis_cache: &HashMap<String, crate::analysis::AnalysisResult>,
) -> Response {
    let uri = &params.text_document.uri;

    let Some(doc) = documents.get(uri) else {
        return Response::new_ok(id, Value::Null);
    };

    let uri_str = uri.as_str();
    let Some(analysis) = analysis_cache.get(uri_str) else {
        return Response::new_ok(id, Value::Null);
    };

    let Some(module) = &analysis.module else {
        return Response::new_ok(id, Value::Null);
    };

    let symbols = extract_document_symbols(module, doc);
    let response = DocumentSymbolResponse::Nested(symbols);
    Response::new_ok(id, serde_json::to_value(response).unwrap_or(Value::Null))
}

/// Handle workspace symbol request.
fn handle_workspace_symbol(
    id: RequestId,
    params: &WorkspaceSymbolParams,
    workspace_index: &WorkspaceIndex,
    documents: &DocumentStore,
    symbol_db: Option<&SymbolDb>,
) -> Response {
    let query = params.query.to_lowercase();
    let mut symbols: Vec<SymbolInformation> = Vec::new();

    // Try to use SymbolDb for searching if available
    if let Some(db) = symbol_db {
        if let Ok(db_symbols) = db.search_workspace_symbols(&query) {
            for ws in db_symbols {
                let range = compute_range_for_symbol(&ws.file_path, ws.span, documents);
                let uri = file_path_to_uri(&ws.file_path);

                #[allow(deprecated)]
                symbols.push(SymbolInformation {
                    name: ws.name,
                    kind: db_kind_to_lsp(&ws.kind),
                    tags: None,
                    deprecated: None,
                    location: Location { uri, range },
                    container_name: Some(ws.module_path),
                });
            }

            let response = WorkspaceSymbolResponse::Flat(symbols);
            return Response::new_ok(id, serde_json::to_value(response).unwrap_or(Value::Null));
        }
    }

    // Fall back to workspace index
    for module_info in workspace_index.all_modules() {
        // Get the document for range calculation
        let doc = documents.get(&module_info.uri);

        for export in &module_info.exports {
            // Filter by query (case-insensitive substring match)
            if !query.is_empty() && !export.name.to_lowercase().contains(&query) {
                continue;
            }

            let range = if let Some(doc) = doc {
                offset_range_to_lsp_range(doc, export.offset as usize, export.end_offset as usize)
            } else {
                // Try to read the file to compute proper range
                if let Some(file_path) = uri_to_path(&module_info.uri) {
                    if let Ok(content) = std::fs::read_to_string(&file_path) {
                        let temp_doc =
                            crate::documents::Document::new(module_info.uri.clone(), 0, content);
                        offset_range_to_lsp_range(
                            &temp_doc,
                            export.offset as usize,
                            export.end_offset as usize,
                        )
                    } else {
                        lsp_types::Range::default()
                    }
                } else {
                    lsp_types::Range::default()
                }
            };

            #[allow(deprecated)] // SymbolInformation::deprecated field is deprecated
            symbols.push(SymbolInformation {
                name: export.name.to_string(),
                kind: symbol_kind_to_lsp(export.kind),
                tags: None,
                deprecated: None,
                location: Location {
                    uri: module_info.uri.clone(),
                    range,
                },
                container_name: Some(module_info.module_path.join(".")),
            });
        }
    }

    let response = WorkspaceSymbolResponse::Flat(symbols);
    Response::new_ok(id, serde_json::to_value(response).unwrap_or(Value::Null))
}

/// Compute a range for a symbol given its file path and span.
fn compute_range_for_symbol(
    file_path: &str,
    span: ambient_engine::ast::Span,
    documents: &DocumentStore,
) -> lsp_types::Range {
    // Try to find the document in the store first
    let path = std::path::Path::new(file_path);
    if let Some(uri) = crate::util::path_to_uri(path) {
        if let Some(doc) = documents.get(&uri) {
            return offset_range_to_lsp_range(doc, span.start as usize, span.end as usize);
        }
    }

    // Try to read the file
    if let Ok(content) = std::fs::read_to_string(file_path) {
        let uri = file_path_to_uri(file_path);
        let temp_doc = crate::documents::Document::new(uri, 0, content);
        return offset_range_to_lsp_range(&temp_doc, span.start as usize, span.end as usize);
    }

    lsp_types::Range::default()
}

/// Convert a file path string to a URI.
fn file_path_to_uri(file_path: &str) -> Uri {
    // Try using our utility function first
    let path = std::path::Path::new(file_path);
    crate::util::path_to_uri(path).unwrap_or_else(|| {
        // Fallback: parse directly
        Uri::from_str(&format!("file://{file_path}"))
            .unwrap_or_else(|_| Uri::from_str("file:///unknown").expect("valid fallback URI"))
    })
}

/// Populate the symbol database from all modules in a package.
///
/// This type-checks each module and stores symbols in the database,
/// enabling workspace-wide symbol search without opening each file.
///
/// Uses smart caching:
/// - Skips modules whose source hash hasn't changed
/// - Cascades invalidation to dependents when exports change
fn populate_symbol_db_from_package(db: &mut SymbolDb, pkg: &PackageInfo) {
    let registry = pkg.build_registry();

    // Track modules whose exports changed (need to invalidate dependents)
    let mut changed_exports: Vec<String> = Vec::new();

    // First pass: analyze modules that have changed
    for parsed_module in pkg.modules.values() {
        let module_path_str = parsed_module.path.to_string();
        let source_hash = compute_source_hash(&parsed_module.source);

        // Skip if source hasn't changed
        if db
            .is_module_up_to_date(&module_path_str, source_hash)
            .unwrap_or(false)
        {
            continue;
        }

        // Type check and update database
        if let Some(export_changed) =
            analyze_and_store_module(db, parsed_module, &registry, &pkg.src_dir)
        {
            if export_changed {
                changed_exports.push(module_path_str);
            }
        }
    }

    // Cascade invalidation: re-analyze dependents of changed modules
    let mut processed: std::collections::HashSet<String> = std::collections::HashSet::new();
    while !changed_exports.is_empty() {
        let mut next_changed: Vec<String> = Vec::new();

        for module_path in &changed_exports {
            // Find all modules that depend on this one
            if let Ok(dependents) = db.get_dependents(module_path) {
                for dep_path in dependents {
                    // Skip if already processed in this cascade
                    if processed.contains(&dep_path) {
                        continue;
                    }
                    processed.insert(dep_path.clone());

                    // Find and re-analyze the dependent module
                    if let Some(parsed_module) = pkg.modules.get(&dep_path) {
                        if let Some(export_changed) =
                            analyze_and_store_module(db, parsed_module, &registry, &pkg.src_dir)
                        {
                            if export_changed {
                                next_changed.push(dep_path);
                            }
                        }
                    }
                }
            }
        }

        changed_exports = next_changed;
    }
}

/// Analyze a single module and store it in the database.
///
/// Returns `Some(true)` if export hash changed, `Some(false)` if not,
/// or `None` if type checking failed.
fn analyze_and_store_module(
    db: &mut SymbolDb,
    parsed_module: &crate::package::ParsedModule,
    registry: &ambient_engine::module_registry::ModuleRegistry,
    src_dir: &std::path::Path,
) -> Option<bool> {
    let module_path = &parsed_module.path;

    // Type check the module
    let result = analyze_with_registry(&parsed_module.source, Some(module_path), Some(registry));

    // If type checking succeeded, populate the database
    let module = result.module.as_ref()?;
    let module_path_str = module_path.to_string();
    let file_path = src_dir.join(module_path.to_file_path());
    let file_path_str = file_path.display().to_string();

    // Compute hashes
    let source_hash = compute_source_hash(&parsed_module.source);
    let export_hash = compute_export_hash(module, &module_path_str);

    // Extract symbols and dependencies
    let symbols = extract_symbols(module, &module_path_str);
    let dependencies = extract_dependencies(module);

    // Build module info and upsert
    let info = ModuleInfo {
        path: module_path_str,
        file_path: file_path_str,
        source_hash,
        export_hash,
        doc: module.doc.as_ref().map(ToString::to_string),
        symbols,
        dependencies,
    };

    db.upsert_module(&info).ok()
}

/// Symbol information from `SymbolDb` for hover.
struct SymbolHoverInfo {
    type_signature: ambient_engine::types::Type,
    doc: Option<String>,
}

/// Look up symbol information from `SymbolDb` for a qualified name.
///
/// Returns type signature and documentation for cross-file symbol lookups.
fn lookup_qname_info(
    qname: &ambient_engine::ast::QualifiedName,
    symbol_db: Option<&SymbolDb>,
) -> Option<SymbolHoverInfo> {
    if qname.path.is_empty() {
        return None;
    }

    let db = symbol_db?;

    // Build qualified name: path.name
    let qualified_name = format!(
        "{}.{}",
        qname
            .path
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>()
            .join("."),
        qname.name
    );

    db.lookup_symbol(&qualified_name)
        .ok()
        .flatten()
        .map(|record| SymbolHoverInfo {
            type_signature: record.type_signature,
            doc: record.doc,
        })
}

/// Convert a database kind string to LSP `SymbolKind`.
fn db_kind_to_lsp(kind: &str) -> LspSymbolKind {
    match kind {
        "function" => LspSymbolKind::FUNCTION,
        "const" => LspSymbolKind::CONSTANT,
        "type_alias" => LspSymbolKind::TYPE_PARAMETER,
        "enum" => LspSymbolKind::ENUM,
        "ability" => LspSymbolKind::INTERFACE,
        _ => LspSymbolKind::VARIABLE,
    }
}

/// Handle semantic tokens request.
fn handle_semantic_tokens(
    id: RequestId,
    params: &SemanticTokensParams,
    documents: &DocumentStore,
    analysis_cache: &HashMap<String, crate::analysis::AnalysisResult>,
) -> Response {
    let uri = &params.text_document.uri;

    let Some(doc) = documents.get(uri) else {
        return Response::new_ok(id, Value::Null);
    };

    let uri_str = uri.as_str();
    let Some(analysis) = analysis_cache.get(uri_str) else {
        return Response::new_ok(id, Value::Null);
    };

    let Some(module) = &analysis.module else {
        return Response::new_ok(id, Value::Null);
    };

    let tokens = extract_semantic_tokens(module, doc);
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
    }
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

/// Convert our `SymbolKind` to LSP `SymbolKind`.
fn symbol_kind_to_lsp(kind: SymbolKind) -> LspSymbolKind {
    match kind {
        SymbolKind::Function => LspSymbolKind::FUNCTION,
        SymbolKind::Const => LspSymbolKind::CONSTANT,
        SymbolKind::TypeAlias => LspSymbolKind::TYPE_PARAMETER,
        SymbolKind::Enum => LspSymbolKind::ENUM,
        SymbolKind::Ability => LspSymbolKind::INTERFACE,
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
    documents: &mut DocumentStore,
    analysis_cache: &mut HashMap<String, crate::analysis::AnalysisResult>,
    workspace_index: &mut WorkspaceIndex,
    package_info: &mut Option<PackageInfo>,
    symbol_db: &mut Option<SymbolDb>,
    connection: &Connection,
) -> anyhow::Result<()> {
    match notif.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params: DidOpenTextDocumentParams = serde_json::from_value(notif.params.clone())?;
            let uri = params.text_document.uri.clone();
            let version = params.text_document.version;
            let text = params.text_document.text.clone();

            documents.open(uri.clone(), version, text.clone());

            // Discover package if not already discovered
            if package_info.is_none() {
                if let Some(mut pkg) = PackageInfo::discover(&uri) {
                    pkg.discover_modules();
                    // Populate workspace index with all discovered modules
                    // This enables go-to-definition for imports
                    pkg.populate_workspace_index(workspace_index);

                    // Initialize the symbol database
                    let db_path = pkg.root.join("build").join("index.db");
                    if let Some(parent) = db_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Ok(mut db) = SymbolDb::open(&db_path) {
                        // Populate database with all discovered modules
                        populate_symbol_db_from_package(&mut db, &pkg);
                        *symbol_db = Some(db);
                    }

                    *package_info = Some(pkg);
                }
            }

            // Analyze with cross-module support if we have a package
            let result = if let Some(pkg) = package_info.as_ref() {
                let module_path = pkg.uri_to_module_path(&uri);
                let registry = pkg.build_registry();
                analyze_with_registry(&text, module_path.as_ref(), Some(&registry))
            } else {
                analyze_with_registry(&text, None, None)
            };

            let diagnostics = collect_diagnostics(documents.get(&uri), &result);
            publish_diagnostics(connection, uri.clone(), diagnostics, version)?;

            // Update workspace index if we have a valid module
            if let Some(ref module) = result.module {
                workspace_index.update(uri.clone(), module);

                // Update package info with the newly parsed module
                if let Some(pkg) = package_info.as_mut() {
                    pkg.update_module(&uri, &text, module.clone());
                }

                // Update symbol database and cascade to dependents
                if let (Some(db), Some(pkg)) = (symbol_db.as_mut(), package_info.as_ref()) {
                    if let Some(module_path) = pkg.uri_to_module_path(&uri) {
                        update_symbol_db(db, &module_path.to_string(), &uri, &text, module, pkg);
                    }
                }
            }

            analysis_cache.insert(uri.as_str().to_string(), result);
        }
        DidChangeTextDocument::METHOD => {
            let params: DidChangeTextDocumentParams = serde_json::from_value(notif.params.clone())?;
            let uri = params.text_document.uri.clone();
            let version = params.text_document.version;

            // We use full sync, so there's exactly one change with the full text.
            if let Some(change) = params.content_changes.into_iter().next() {
                documents.update(&uri, version, change.text.clone());

                // Re-analyze with cross-module support
                if let Some(doc) = documents.get(&uri) {
                    let result = if let Some(pkg) = package_info.as_ref() {
                        let module_path = pkg.uri_to_module_path(&uri);
                        let registry = pkg.build_registry();
                        analyze_with_registry(&doc.text, module_path.as_ref(), Some(&registry))
                    } else {
                        analyze_with_registry(&doc.text, None, None)
                    };

                    let diagnostics = collect_diagnostics(Some(doc), &result);
                    publish_diagnostics(connection, uri.clone(), diagnostics, version)?;

                    // Update workspace index if we have a valid module
                    if let Some(ref module) = result.module {
                        workspace_index.update(uri.clone(), module);

                        // Update package info with the newly parsed module
                        if let Some(pkg) = package_info.as_mut() {
                            pkg.update_module(&uri, &doc.text, module.clone());
                        }

                        // Update symbol database
                        if let (Some(db), Some(pkg)) = (symbol_db.as_mut(), package_info.as_ref()) {
                            if let Some(module_path) = pkg.uri_to_module_path(&uri) {
                                update_symbol_db(
                                    db,
                                    &module_path.to_string(),
                                    &uri,
                                    &doc.text,
                                    module,
                                    pkg,
                                );
                            }
                        }
                    }

                    analysis_cache.insert(uri.as_str().to_string(), result);
                }
            }
        }
        DidCloseTextDocument::METHOD => {
            let params: DidCloseTextDocumentParams = serde_json::from_value(notif.params.clone())?;
            let uri = params.text_document.uri;

            documents.close(&uri);
            analysis_cache.remove(uri.as_str());
            workspace_index.remove(&uri);

            // Clear diagnostics.
            publish_diagnostics(connection, uri, Vec::new(), 0)?;
        }
        _ => {
            // Unknown notification, ignore.
        }
    }

    Ok(())
}

/// Collect diagnostics from an analysis result.
fn collect_diagnostics(
    doc: Option<&crate::documents::Document>,
    result: &crate::analysis::AnalysisResult,
) -> Vec<Diagnostic> {
    let Some(doc) = doc else {
        return Vec::new();
    };

    let mut diagnostics = Vec::new();

    if let Some(parse_error) = &result.parse_error {
        diagnostics.push(parse_error_to_diagnostic(doc, parse_error));
    }

    for type_error in &result.type_errors {
        diagnostics.push(type_error_to_diagnostic(doc, type_error));
    }

    diagnostics
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

/// Update the symbol database with a typed module and cascade to dependents.
fn update_symbol_db(
    db: &mut SymbolDb,
    module_path: &str,
    uri: &Uri,
    source: &str,
    module: &Module,
    pkg: &PackageInfo,
) {
    // Get the file path relative to src/
    let file_path = uri_to_path(uri)
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    // Compute hashes
    let source_hash = compute_source_hash(source);
    let export_hash = compute_export_hash(module, module_path);

    // Extract symbols and dependencies
    let symbols = extract_symbols(module, module_path);
    let dependencies = extract_dependencies(module);

    // Build module info and upsert
    let info = ModuleInfo {
        path: module_path.to_string(),
        file_path,
        source_hash,
        export_hash,
        doc: module.doc.as_ref().map(ToString::to_string),
        symbols,
        dependencies,
    };

    // Upsert and check if exports changed
    let export_changed = db.upsert_module(&info).unwrap_or(false);

    // If exports changed, cascade invalidation to dependents
    if export_changed {
        cascade_invalidation(db, module_path, pkg);
    }
}

/// Cascade invalidation to all modules that depend on the changed module.
fn cascade_invalidation(db: &mut SymbolDb, changed_module: &str, pkg: &PackageInfo) {
    let registry = pkg.build_registry();
    let mut changed_exports = vec![changed_module.to_string()];
    let mut processed: std::collections::HashSet<String> = std::collections::HashSet::new();
    processed.insert(changed_module.to_string());

    while !changed_exports.is_empty() {
        let mut next_changed: Vec<String> = Vec::new();

        for module_path in &changed_exports {
            if let Ok(dependents) = db.get_dependents(module_path) {
                for dep_path in dependents {
                    if processed.contains(&dep_path) {
                        continue;
                    }
                    processed.insert(dep_path.clone());

                    if let Some(parsed_module) = pkg.modules.get(&dep_path) {
                        if let Some(export_changed) =
                            analyze_and_store_module(db, parsed_module, &registry, &pkg.src_dir)
                        {
                            if export_changed {
                                next_changed.push(dep_path);
                            }
                        }
                    }
                }
            }
        }

        changed_exports = next_changed;
    }
}

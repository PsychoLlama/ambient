//! LSP server implementation for the Ambient language.

use std::collections::HashMap;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, Request as _,
    WorkspaceSymbolRequest,
};
use lsp_types::{
    CompletionOptions, CompletionParams, CompletionResponse, Diagnostic,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, Location, MarkedString, OneOf, PublishDiagnosticsParams,
    ServerCapabilities, SymbolInformation, SymbolKind as LspSymbolKind, TextDocumentSyncCapability,
    TextDocumentSyncKind, Uri, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use serde_json::Value;

use ambient_engine::ast::{ItemKind, Module};

use crate::analysis::{
    analyze_with_registry, find_definition_cross_file, find_expr_at_offset, format_type,
};
use crate::completions::{get_completions, CompletionContext};
use crate::convert::{
    offset_range_to_lsp_range, parse_error_to_diagnostic, type_error_to_diagnostic,
};
use crate::documents::DocumentStore;
use crate::package::PackageInfo;
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

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }

                let response = handle_request(&req, &documents, &analysis_cache, &workspace_index);
                connection.sender.send(Message::Response(response))?;
            }
            Message::Notification(notif) => {
                handle_notification(
                    &notif,
                    &mut documents,
                    &mut analysis_cache,
                    &mut workspace_index,
                    &mut package_info,
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
) -> Response {
    let id = req.id.clone();

    match req.method.as_str() {
        HoverRequest::METHOD => {
            let params = match parse_params(&req.params, &id) {
                Ok(p) => p,
                Err(e) => return e,
            };
            handle_hover(id, &params, documents, analysis_cache)
        }
        GotoDefinition::METHOD => {
            let params = match parse_params(&req.params, &id) {
                Ok(p) => p,
                Err(e) => return e,
            };
            handle_goto_definition(id, &params, documents, analysis_cache, workspace_index)
        }
        Completion::METHOD => {
            let params = match parse_params(&req.params, &id) {
                Ok(p) => p,
                Err(e) => return e,
            };
            handle_completion(id, &params, documents, analysis_cache)
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
            handle_workspace_symbol(id, &params, workspace_index, documents)
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
    let Some(expr) = find_expr_at_offset(module, offset as u32) else {
        return Response::new_ok(id, Value::Null);
    };

    // Get type information if available.
    let type_info = if let Some(ty) = &expr.ty {
        format_type(ty)
    } else {
        "unknown".to_string()
    };

    // Build hover content.
    let content = match &expr.kind {
        ambient_engine::ast::ExprKind::Local(local_id) => {
            format!("```ambient\nlocal_{local_id}: {type_info}\n```")
        }
        ambient_engine::ast::ExprKind::Name(qname) => {
            format!("```ambient\n{}: {type_info}\n```", qname.name)
        }
        ambient_engine::ast::ExprKind::Bool(b) => {
            format!("```ambient\n{b}: {type_info}\n```")
        }
        ambient_engine::ast::ExprKind::Number(n) => {
            format!("```ambient\n{n}: {type_info}\n```")
        }
        ambient_engine::ast::ExprKind::String(s) => {
            format!("```ambient\n\"{s}\": {type_info}\n```")
        }
        _ => {
            format!("```ambient\n{type_info}\n```")
        }
    };

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
    documents: &DocumentStore,
    analysis_cache: &HashMap<String, crate::analysis::AnalysisResult>,
    workspace_index: &WorkspaceIndex,
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
    let Some(def_result) = find_definition_cross_file(module, offset as u32, uri, workspace_index) else {
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
    let items = get_completions(&ctx, module);

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
) -> Response {
    let query = params.query.to_lowercase();
    let mut symbols: Vec<SymbolInformation> = Vec::new();

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

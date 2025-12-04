//! LSP server implementation for the Ambient language.

use std::collections::HashMap;
use std::io::{Read, Write};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{Completion, GotoDefinition, HoverRequest, Request as _};
use lsp_types::{
    CompletionOptions, CompletionParams, CompletionResponse, Diagnostic,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents, HoverParams,
    HoverProviderCapability, InitializeParams, InitializeResult, Location, MarkedString, OneOf,
    PublishDiagnosticsParams, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    Uri,
};
use serde_json::Value;

use crate::analysis::{analyze, find_definition, find_expr_at_offset, format_type};
use crate::completions::{get_completions, CompletionContext};
use crate::convert::{
    offset_range_to_lsp_range, parse_error_to_diagnostic, type_error_to_diagnostic,
};
use crate::documents::DocumentStore;

/// Run the LSP server.
///
/// # Errors
///
/// Returns an error if the server fails to start or communicate with the client.
pub fn run_server<R, W>(_reader: R, _writer: W) -> anyhow::Result<()>
where
    R: Read,
    W: Write,
{
    let (connection, io_threads) = Connection::stdio();

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

    io_threads.join()?;
    Ok(())
}

/// The main server loop.
fn main_loop(connection: &Connection) -> anyhow::Result<()> {
    let mut documents = DocumentStore::new();
    // Use String keys instead of Uri to avoid mutable_key_type warning (Uri has interior mutability).
    let mut analysis_cache: HashMap<String, crate::analysis::AnalysisResult> = HashMap::new();

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }

                let response = handle_request(&req, &documents, &analysis_cache);
                connection.sender.send(Message::Response(response))?;
            }
            Message::Notification(notif) => {
                handle_notification(&notif, &mut documents, &mut analysis_cache, connection)?;
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
            handle_goto_definition(id, &params, documents, analysis_cache)
        }
        Completion::METHOD => {
            let params = match parse_params(&req.params, &id) {
                Ok(p) => p,
                Err(e) => return e,
            };
            handle_completion(id, &params, documents, analysis_cache)
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
    let Some(def_result) = find_definition(module, offset as u32) else {
        return Response::new_ok(id, Value::Null);
    };

    let location = Location {
        uri: uri.clone(),
        range: offset_range_to_lsp_range(
            doc,
            def_result.span.start as usize,
            def_result.span.end as usize,
        ),
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

/// Handle an incoming notification.
fn handle_notification(
    notif: &Notification,
    documents: &mut DocumentStore,
    analysis_cache: &mut HashMap<String, crate::analysis::AnalysisResult>,
    connection: &Connection,
) -> anyhow::Result<()> {
    match notif.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params: DidOpenTextDocumentParams = serde_json::from_value(notif.params.clone())?;
            let uri = params.text_document.uri.clone();
            let version = params.text_document.version;
            let text = params.text_document.text.clone();

            documents.open(uri.clone(), version, text.clone());

            // Analyze and publish diagnostics.
            let result = analyze(&text);
            let diagnostics = collect_diagnostics(documents.get(&uri), &result);
            publish_diagnostics(connection, uri.clone(), diagnostics, version)?;
            analysis_cache.insert(uri.as_str().to_string(), result);
        }
        DidChangeTextDocument::METHOD => {
            let params: DidChangeTextDocumentParams = serde_json::from_value(notif.params.clone())?;
            let uri = params.text_document.uri.clone();
            let version = params.text_document.version;

            // We use full sync, so there's exactly one change with the full text.
            if let Some(change) = params.content_changes.into_iter().next() {
                documents.update(&uri, version, change.text.clone());

                // Re-analyze and publish diagnostics.
                if let Some(doc) = documents.get(&uri) {
                    let result = analyze(&doc.text);
                    let diagnostics = collect_diagnostics(Some(doc), &result);
                    publish_diagnostics(connection, uri.clone(), diagnostics, version)?;
                    analysis_cache.insert(uri.as_str().to_string(), result);
                }
            }
        }
        DidCloseTextDocument::METHOD => {
            let params: DidCloseTextDocumentParams = serde_json::from_value(notif.params.clone())?;
            let uri = params.text_document.uri;

            documents.close(&uri);
            analysis_cache.remove(uri.as_str());

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

//! In-process LSP test client using `Connection::memory()`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::notification::{
    DidChangeTextDocument, DidOpenTextDocument, Initialized, Notification as NotificationTrait,
    PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, Initialize,
    PrepareRenameRequest, References, Rename, Request as RequestTrait, SemanticTokensFullRequest,
    Shutdown,
};
use lsp_types::{
    ClientCapabilities, CompletionItem, CompletionParams, CompletionResponse, Diagnostic,
    DidChangeTextDocumentParams, DidOpenTextDocumentParams, DocumentSymbol, DocumentSymbolParams,
    DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams,
    InitializeParams, InitializeResult, Location, PartialResultParams, Position,
    PrepareRenameResponse, ReferenceContext, ReferenceParams, RenameParams, SemanticToken,
    SemanticTokensParams, SemanticTokensResult, TextDocumentContentChangeEvent,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, Uri,
    VersionedTextDocumentIdentifier, WorkDoneProgressParams, WorkspaceEdit,
};

use crate::run_server_with_connection;

/// An in-process LSP test client.
///
/// Spawns the LSP server in a background thread and communicates via memory channels.
pub struct TestClient {
    /// The client connection.
    connection: Connection,
    /// The server thread handle.
    server_thread: Option<JoinHandle<anyhow::Result<()>>>,
    /// Request ID counter.
    next_id: i32,
    /// Monotonic document version counter for edits (`didChange`).
    next_doc_version: i32,
    /// Collected diagnostics by URI.
    diagnostics: Arc<Mutex<HashMap<String, Vec<Diagnostic>>>>,
}

impl TestClient {
    /// Create a new test client and spawn the LSP server.
    #[must_use]
    pub fn new() -> Self {
        let (server_conn, client_conn) = Connection::memory();

        let server_thread = std::thread::spawn(move || run_server_with_connection(server_conn));

        let mut client = Self {
            connection: client_conn,
            server_thread: Some(server_thread),
            next_id: 1,
            next_doc_version: 2,
            diagnostics: Arc::new(Mutex::new(HashMap::new())),
        };

        // Perform initialization handshake
        client.initialize();

        client
    }

    /// Perform the LSP initialization handshake.
    #[allow(deprecated)]
    fn initialize(&mut self) {
        let params = InitializeParams {
            process_id: None,
            root_path: None,
            root_uri: None,
            initialization_options: None,
            capabilities: ClientCapabilities::default(),
            trace: None,
            workspace_folders: None,
            client_info: None,
            locale: None,
            work_done_progress_params: lsp_types::WorkDoneProgressParams {
                work_done_token: None,
            },
        };

        let _result: InitializeResult = self.send_request::<Initialize>(params);

        // Send initialized notification
        self.send_notification::<Initialized>(lsp_types::InitializedParams {});
    }

    /// Send a request and wait for the response.
    fn send_request<R: RequestTrait>(&mut self, params: R::Params) -> R::Result
    where
        R::Params: serde::Serialize,
        R::Result: serde::de::DeserializeOwned,
    {
        let id = RequestId::from(self.next_id);
        self.next_id += 1;

        let request = Request::new(
            id.clone(),
            R::METHOD.to_string(),
            serde_json::to_value(params).unwrap(),
        );

        self.connection
            .sender
            .send(Message::Request(request))
            .expect("Failed to send request");

        // Wait for the response, processing any notifications along the way
        loop {
            let msg = self
                .connection
                .receiver
                .recv()
                .expect("Failed to receive message");

            match msg {
                Message::Response(response) => {
                    if response.id == id {
                        if let Some(err) = response.error {
                            panic!("LSP request failed: {:?}", err);
                        }
                        return serde_json::from_value(response.result.unwrap_or_default())
                            .expect("Failed to parse response");
                    }
                }
                Message::Notification(notif) => {
                    self.handle_notification(notif);
                }
                Message::Request(_) => {
                    // Server-initiated requests - ignore for now
                }
            }
        }
    }

    /// Send a request and wait for the response, surfacing a server error as
    /// `Err(message)` instead of panicking.
    fn send_request_result<R: RequestTrait>(
        &mut self,
        params: R::Params,
    ) -> Result<R::Result, String>
    where
        R::Params: serde::Serialize,
        R::Result: serde::de::DeserializeOwned,
    {
        let id = RequestId::from(self.next_id);
        self.next_id += 1;

        let request = Request::new(
            id.clone(),
            R::METHOD.to_string(),
            serde_json::to_value(params).unwrap(),
        );

        self.connection
            .sender
            .send(Message::Request(request))
            .expect("Failed to send request");

        loop {
            let msg = self
                .connection
                .receiver
                .recv()
                .expect("Failed to receive message");

            match msg {
                Message::Response(response) => {
                    if response.id == id {
                        if let Some(err) = response.error {
                            return Err(err.message);
                        }
                        return Ok(serde_json::from_value(response.result.unwrap_or_default())
                            .expect("Failed to parse response"));
                    }
                }
                Message::Notification(notif) => self.handle_notification(notif),
                Message::Request(_) => {}
            }
        }
    }

    /// Send a notification (no response expected).
    fn send_notification<N: NotificationTrait>(&self, params: N::Params)
    where
        N::Params: serde::Serialize,
    {
        let notification =
            Notification::new(N::METHOD.to_string(), serde_json::to_value(params).unwrap());

        self.connection
            .sender
            .send(Message::Notification(notification))
            .expect("Failed to send notification");
    }

    /// Handle incoming notifications from the server.
    fn handle_notification(&self, notif: Notification) {
        if notif.method == PublishDiagnostics::METHOD {
            let params: lsp_types::PublishDiagnosticsParams =
                serde_json::from_value(notif.params).expect("Failed to parse diagnostics");

            let mut diags = self.diagnostics.lock().unwrap();
            diags.insert(params.uri.to_string(), params.diagnostics);
        }
    }

    /// Process any pending notifications (non-blocking).
    pub fn process_notifications(&self) {
        while let Ok(msg) = self.connection.receiver.try_recv() {
            if let Message::Notification(notif) = msg {
                self.handle_notification(notif);
            }
        }
    }

    /// Open a document in the LSP server.
    pub fn open_document(&mut self, uri: Uri, text: &str) {
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id: "ambient".to_string(),
                version: 1,
                text: text.to_string(),
            },
        };

        self.send_notification::<DidOpenTextDocument>(params);

        // Give the server time to process and send diagnostics
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.process_notifications();
    }

    /// Request hover information at a position.
    pub fn hover(&mut self, uri: &Uri, line: u32, character: u32) -> Option<Hover> {
        let params = HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position { line, character },
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams {
                work_done_token: None,
            },
        };

        self.send_request::<HoverRequest>(params)
    }

    /// Request go-to-definition at a position.
    pub fn goto_definition(&mut self, uri: &Uri, line: u32, character: u32) -> Vec<Location> {
        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position { line, character },
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams {
                work_done_token: None,
            },
            partial_result_params: lsp_types::PartialResultParams {
                partial_result_token: None,
            },
        };

        let response: Option<GotoDefinitionResponse> = self.send_request::<GotoDefinition>(params);

        match response {
            Some(GotoDefinitionResponse::Scalar(loc)) => vec![loc],
            Some(GotoDefinitionResponse::Array(locs)) => locs,
            Some(GotoDefinitionResponse::Link(links)) => links
                .into_iter()
                .map(|l| Location {
                    uri: l.target_uri,
                    range: l.target_selection_range,
                })
                .collect(),
            None => vec![],
        }
    }

    /// Edit an already-open document by sending a full-text `didChange`.
    ///
    /// Uses full document sync (one change event carrying the whole text),
    /// matching the server's expectation.
    pub fn change_document(&mut self, uri: &Uri, text: &str) {
        let version = self.next_doc_version;
        self.next_doc_version += 1;

        let params = DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.to_string(),
            }],
        };

        self.send_notification::<DidChangeTextDocument>(params);

        // Give the server time to re-analyze and send diagnostics.
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.process_notifications();
    }

    /// Request find-references at a position.
    ///
    /// Mirrors [`goto_definition`](Self::goto_definition), threading through
    /// the `include_declaration` flag of the reference context.
    pub fn references(
        &mut self,
        uri: &Uri,
        line: u32,
        character: u32,
        include_declaration: bool,
    ) -> Vec<Location> {
        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position { line, character },
            },
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
            partial_result_params: PartialResultParams {
                partial_result_token: None,
            },
            context: ReferenceContext {
                include_declaration,
            },
        };

        let response: Option<Vec<Location>> = self.send_request::<References>(params);
        response.unwrap_or_default()
    }

    /// Request prepare-rename at a position. `None` means the server rejected
    /// the position as non-renameable.
    pub fn prepare_rename(
        &mut self,
        uri: &Uri,
        line: u32,
        character: u32,
    ) -> Option<PrepareRenameResponse> {
        let params = TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: Position { line, character },
        };
        self.send_request::<PrepareRenameRequest>(params)
    }

    /// Request a rename, expecting success. Panics if the server returns an
    /// error (use [`try_rename`](Self::try_rename) for the rejection path).
    pub fn rename(
        &mut self,
        uri: &Uri,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> Option<WorkspaceEdit> {
        self.try_rename(uri, line, character, new_name)
            .expect("rename request failed")
    }

    /// Request a rename, surfacing a server-side rejection as `Err(message)`.
    pub fn try_rename(
        &mut self,
        uri: &Uri,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> Result<Option<WorkspaceEdit>, String> {
        let params = RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position { line, character },
            },
            new_name: new_name.to_string(),
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
        };
        self.send_request_result::<Rename>(params)
    }

    /// Request semantic tokens for the whole document.
    pub fn semantic_tokens(&mut self, uri: &Uri) -> Vec<SemanticToken> {
        let params = SemanticTokensParams {
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
            partial_result_params: PartialResultParams {
                partial_result_token: None,
            },
            text_document: TextDocumentIdentifier { uri: uri.clone() },
        };

        let response: Option<SemanticTokensResult> =
            self.send_request::<SemanticTokensFullRequest>(params);

        match response {
            Some(SemanticTokensResult::Tokens(tokens)) => tokens.data,
            Some(SemanticTokensResult::Partial(partial)) => partial.data,
            None => vec![],
        }
    }

    /// Request document symbols. Returns the flat/nested symbol list.
    pub fn document_symbol(&mut self, uri: &Uri) -> Vec<DocumentSymbol> {
        let params = DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
            partial_result_params: PartialResultParams {
                partial_result_token: None,
            },
        };

        let response: Option<DocumentSymbolResponse> =
            self.send_request::<DocumentSymbolRequest>(params);

        match response {
            Some(DocumentSymbolResponse::Nested(symbols)) => symbols,
            // The server only emits the nested form; flat responses carry no
            // hierarchy, so surface them as an empty list rather than inventing one.
            Some(DocumentSymbolResponse::Flat(_)) | None => vec![],
        }
    }

    /// Request completions at a position.
    pub fn complete(&mut self, uri: &Uri, line: u32, character: u32) -> Vec<CompletionItem> {
        let params = CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position { line, character },
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams {
                work_done_token: None,
            },
            partial_result_params: lsp_types::PartialResultParams {
                partial_result_token: None,
            },
            context: None,
        };

        let response: Option<CompletionResponse> = self.send_request::<Completion>(params);

        match response {
            Some(CompletionResponse::Array(items)) => items,
            Some(CompletionResponse::List(list)) => list.items,
            None => vec![],
        }
    }

    /// Get diagnostics for a URI.
    pub fn get_diagnostics(&self, uri: &Uri) -> Vec<Diagnostic> {
        let diags = self.diagnostics.lock().unwrap();
        diags.get(uri.as_str()).cloned().unwrap_or_default()
    }

    /// Shutdown the LSP server gracefully.
    pub fn shutdown(mut self) {
        // Send shutdown request
        let _: () = self.send_request::<Shutdown>(());

        // Send exit notification
        self.send_notification::<lsp_types::notification::Exit>(());

        // Wait for the server thread to finish
        if let Some(handle) = self.server_thread.take() {
            let _ = handle.join();
        }
    }
}

impl Default for TestClient {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TestClient {
    fn drop(&mut self) {
        // If the client is dropped without shutdown, try to clean up
        if self.server_thread.is_some() {
            // Try to send shutdown - ignore errors
            let _ = self.connection.sender.send(Message::Request(Request::new(
                RequestId::from(9999),
                Shutdown::METHOD.to_string(),
                serde_json::Value::Null,
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_initialization() {
        let client = TestClient::new();
        client.shutdown();
    }

    #[test]
    fn test_open_document() {
        let mut client = TestClient::new();
        let uri: Uri = "file:///test.ab".parse().unwrap();
        client.open_document(uri, "fn foo() { 42 }");
        client.shutdown();
    }
}

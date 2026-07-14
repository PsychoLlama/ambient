//! In-process LSP test client that drives the server's request and
//! notification handlers directly against an in-memory [`ServerState`].
//!
//! There is no background thread, no channel, no temp directory, and no
//! barrier round-trip: `open_document`/`change_document` call
//! [`handle_notification`] and read the diagnostics it returns; every query
//! (`hover`, `references`, …) builds a [`Request`] and calls
//! [`handle_request`]. Everything is synchronous and deterministic. The one
//! test that still exercises the real transport loop lives in
//! `tests/transport.rs`.

use std::collections::HashMap;
use std::path::PathBuf;

use lsp_server::{Notification, Request, RequestId};
use lsp_types::notification::{
    DidChangeTextDocument, DidOpenTextDocument, Notification as NotificationTrait,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, PrepareRenameRequest,
    References, Rename, Request as RequestTrait, SemanticTokensFullRequest,
};
use lsp_types::{
    CompletionItem, CompletionParams, CompletionResponse, Diagnostic, DidChangeTextDocumentParams,
    DidOpenTextDocumentParams, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse,
    GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams, Location,
    PartialResultParams, Position, PrepareRenameResponse, ReferenceContext, ReferenceParams,
    RenameParams, SemanticToken, SemanticTokensParams, SemanticTokensResult,
    TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Uri, VersionedTextDocumentIdentifier, WorkDoneProgressParams,
    WorkspaceEdit,
};

use ambient_analysis::package::AnalysisPackage;
use ambient_analysis::session::AnalysisSession;

use crate::server::{ServerState, handle_notification, handle_request};

/// The notional root every in-memory harness package lives under. No file is
/// ever read from it; it only anchors the `file://` uris and the `src`-prefix
/// path arithmetic module resolution performs.
const VIRTUAL_ROOT: &str = "/ambient-lsp-test";

/// A shared temp base for the harness's materialized core-source cache — kept
/// off the user's real cache dir and out of `$HOME`. Safe to share across tests
/// (even parallel ones): materialization is content-addressed and write-once,
/// so every client resolves the same tree and never interferes.
fn harness_core_cache_base() -> std::path::PathBuf {
    std::env::temp_dir().join("ambient-lsp-test-core-cache")
}

/// An in-process LSP test client backed by a live [`ServerState`].
pub struct TestClient {
    /// The server state the handlers read and mutate.
    state: ServerState,
    /// Request ID counter (handlers ignore the value, but a fresh id per call
    /// keeps the synthesized requests distinct).
    next_id: i32,
    /// Monotonic document version counter for edits (`didChange`).
    next_doc_version: i32,
    /// Latest published diagnostics by URI string, folded from the updates the
    /// notification handlers return.
    diagnostics: HashMap<String, Vec<Diagnostic>>,
    /// The package `src` directory the harness's uris are rooted at.
    src_dir: PathBuf,
}

impl TestClient {
    /// A client with no package: every opened document analyzes as a
    /// stand-alone package root, exactly like `ambient check` on a bare file.
    /// Single-file [`super::LspTest`] tests use this.
    #[must_use]
    pub fn new() -> Self {
        let mut state = ServerState::new();
        state.core_cache_base = Some(harness_core_cache_base());
        Self {
            state,
            next_id: 1,
            next_doc_version: 2,
            diagnostics: HashMap::new(),
            src_dir: PathBuf::from(VIRTUAL_ROOT).join("src"),
        }
    }

    /// A client over an in-memory package built from `src`-relative files
    /// (`utils.ab`, `collections/main.ab`). Discovers nothing from disk: the
    /// [`AnalysisSession`] is constructed up front and injected, so opening a
    /// document routes through the same reanalysis code production uses after
    /// on-disk discovery. `name` is the package (workspace) name.
    #[must_use]
    pub fn with_package(name: &str, files: &[(&str, &str)]) -> Self {
        let root = PathBuf::from(VIRTUAL_ROOT);
        let src_dir = root.join("src");
        let mut package = AnalysisPackage::empty(root, src_dir.clone());
        package.package_name = name.to_string();
        for (rel, content) in files {
            package.insert_module_at_path(rel, (*content).to_string());
        }
        let mut state = ServerState::new();
        state.core_cache_base = Some(harness_core_cache_base());
        state.session = Some(AnalysisSession::new(package));
        Self {
            state,
            next_id: 1,
            next_doc_version: 2,
            diagnostics: HashMap::new(),
            src_dir,
        }
    }

    /// The `file://` uri for a `src`-relative path in this client's package.
    #[must_use]
    pub fn uri(&self, src_relative: &str) -> Uri {
        let path = self.src_dir.join(src_relative);
        format!("file://{}", path.to_string_lossy())
            .parse()
            .expect("valid uri")
    }

    /// The in-memory analysis package, when this is a package client.
    #[must_use]
    pub fn package(&self) -> Option<&AnalysisPackage> {
        self.state.session.as_ref().map(AnalysisSession::package)
    }

    /// Feed a notification through the real handler, folding the diagnostics it
    /// returns into the per-uri map (`reanalyze` republishes every open
    /// document, so a single edit refreshes them all).
    fn dispatch_notification<N: NotificationTrait>(&mut self, params: N::Params)
    where
        N::Params: serde::Serialize,
    {
        let notif = Notification::new(N::METHOD.to_string(), serde_json::to_value(params).unwrap());
        let updates = handle_notification(&notif, &mut self.state).expect("notification handled");
        for update in updates {
            self.diagnostics
                .insert(update.uri.to_string(), update.diagnostics);
        }
    }

    /// Send a request through the handler and parse the response, panicking on
    /// a server-side error.
    fn send_request<R: RequestTrait>(&mut self, params: R::Params) -> R::Result
    where
        R::Params: serde::Serialize,
        R::Result: serde::de::DeserializeOwned,
    {
        self.send_request_result::<R>(params)
            .unwrap_or_else(|err| panic!("LSP request failed: {err}"))
    }

    /// Send a request through the handler, surfacing a server error as
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
            id,
            R::METHOD.to_string(),
            serde_json::to_value(params).unwrap(),
        );
        let response = handle_request(&request, &self.state);
        if let Some(err) = response.error {
            return Err(err.message);
        }
        Ok(serde_json::from_value(response.result.unwrap_or_default())
            .expect("Failed to parse response"))
    }

    /// Open a document in the server.
    pub fn open_document(&mut self, uri: Uri, text: &str) {
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id: "ambient".to_string(),
                version: 1,
                text: text.to_string(),
            },
        };
        self.dispatch_notification::<DidOpenTextDocument>(params);
    }

    /// Edit an already-open document via a full-text `didChange`.
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
        self.dispatch_notification::<DidChangeTextDocument>(params);
    }

    /// Request hover information at a position.
    pub fn hover(&mut self, uri: &Uri, line: u32, character: u32) -> Option<Hover> {
        let params = HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position { line, character },
            },
            work_done_progress_params: WorkDoneProgressParams {
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
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
            partial_result_params: PartialResultParams {
                partial_result_token: None,
            },
        };

        match self.send_request::<GotoDefinition>(params) {
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

        match self.send_request::<SemanticTokensFullRequest>(params) {
            Some(SemanticTokensResult::Tokens(tokens)) => tokens.data,
            Some(SemanticTokensResult::Partial(partial)) => partial.data,
            None => vec![],
        }
    }

    /// Request document symbols. Returns the nested symbol list.
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

        match self.send_request::<DocumentSymbolRequest>(params) {
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
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
            partial_result_params: PartialResultParams {
                partial_result_token: None,
            },
            context: None,
        };

        match self.send_request::<Completion>(params) {
            Some(CompletionResponse::Array(items)) => items,
            Some(CompletionResponse::List(list)) => list.items,
            None => vec![],
        }
    }

    /// Get the latest published diagnostics for a URI.
    #[must_use]
    pub fn get_diagnostics(&self, uri: &Uri) -> Vec<Diagnostic> {
        self.diagnostics
            .get(uri.as_str())
            .cloned()
            .unwrap_or_default()
    }

    /// Shut the client down. A no-op now that there is no thread or connection
    /// to tear down — kept so existing tests read the same.
    pub fn shutdown(self) {}
}

impl Default for TestClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `file://` uri for a single-file (no-package) document. Discovery is
    /// skipped because no `ambient.toml` sits above the virtual root.
    fn single_file_uri() -> Uri {
        "inmemory:///test.ab".parse().unwrap()
    }

    #[test]
    fn open_document_publishes_diagnostics() {
        let mut client = TestClient::new();
        let uri = single_file_uri();
        client.open_document(uri.clone(), "fn bad(): String { 42 }");
        assert!(
            !client.get_diagnostics(&uri).is_empty(),
            "a type error should be published"
        );
    }

    #[test]
    fn clean_document_has_no_diagnostics() {
        let mut client = TestClient::new();
        let uri = single_file_uri();
        client.open_document(uri.clone(), "fn foo() { 42 }");
        assert!(client.get_diagnostics(&uri).is_empty());
    }
}

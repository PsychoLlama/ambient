//! The one end-to-end transport test.
//!
//! Every other LSP test drives the request/notification handlers directly
//! against an in-memory `ServerState`. This one pins the *production loop*:
//! it runs `run_server_with_connection` on a background thread over
//! `Connection::memory()`, performs the real `initialize` handshake, opens a
//! document, reads the published diagnostics, requests a hover, and shuts down
//! — the wiring the direct-call harness deliberately skips. Keep exactly one
//! test here; behavioral coverage belongs on the fast, deterministic path.

use std::thread;

use ambient_lsp::run_server_with_connection;
use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::notification::{
    DidOpenTextDocument, Initialized, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{HoverRequest, Initialize, Shutdown};
use lsp_types::{
    ClientCapabilities, DidOpenTextDocumentParams, Hover, HoverParams, InitializeParams,
    InitializeResult, InitializedParams, Position, PublishDiagnosticsParams,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, Uri,
    WorkDoneProgressParams,
};
use tempfile::TempDir;

/// Drive one round trip: send a request, then drain messages until its
/// response arrives, collecting any diagnostics seen along the way.
fn request<R: lsp_types::request::Request>(
    conn: &Connection,
    id: i32,
    params: R::Params,
    diagnostics: &mut Vec<lsp_types::Diagnostic>,
) -> R::Result
where
    R::Params: serde::Serialize,
    R::Result: serde::de::DeserializeOwned,
{
    let request_id = RequestId::from(id);
    conn.sender
        .send(Message::Request(Request::new(
            request_id.clone(),
            R::METHOD.to_string(),
            serde_json::to_value(params).unwrap(),
        )))
        .unwrap();

    loop {
        match conn.receiver.recv().unwrap() {
            Message::Response(response) if response.id == request_id => {
                assert!(
                    response.error.is_none(),
                    "request failed: {:?}",
                    response.error
                );
                return serde_json::from_value(response.result.unwrap_or_default()).unwrap();
            }
            Message::Notification(notif) if notif.method == PublishDiagnostics::METHOD => {
                let params: PublishDiagnosticsParams =
                    serde_json::from_value(notif.params).unwrap();
                *diagnostics = params.diagnostics;
            }
            _ => {}
        }
    }
}

fn notify<N: lsp_types::notification::Notification>(conn: &Connection, params: N::Params)
where
    N::Params: serde::Serialize,
{
    conn.sender
        .send(Message::Notification(Notification::new(
            N::METHOD.to_string(),
            serde_json::to_value(params).unwrap(),
        )))
        .unwrap();
}

#[test]
#[allow(deprecated)] // InitializeParams has deprecated fields we default
fn full_transport_roundtrip() {
    // A real on-disk package so `didOpen` discovers it exactly as production.
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    std::fs::write(
        root.join("ambient.toml"),
        "[package]\nname = \"smoke\"\nversion = \"0.1.0\"\n\n[build]\nsrc = \"src\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let main = root.join("src/main.ab");
    std::fs::write(&main, "fn bad(): String { 42 }\n").unwrap();
    let uri: Uri = format!("file://{}", main.display()).parse().unwrap();

    let (server_conn, client) = Connection::memory();
    let server = thread::spawn(move || run_server_with_connection(server_conn));

    let mut diagnostics = Vec::new();

    // Handshake.
    let init: InitializeResult = request::<Initialize>(
        &client,
        1,
        InitializeParams {
            capabilities: ClientCapabilities::default(),
            ..Default::default()
        },
        &mut diagnostics,
    );
    assert_eq!(init.server_info.unwrap().name, "ambient-lsp");
    notify::<Initialized>(&client, InitializedParams {});

    // Open the document; the server publishes diagnostics for the type error.
    notify::<DidOpenTextDocument>(
        &client,
        DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "ambient".to_string(),
                version: 1,
                text: "fn bad(): String { 42 }\n".to_string(),
            },
        },
    );

    // A hover round-trip doubles as a barrier: by the time its response
    // arrives, the earlier `didOpen`'s diagnostics have been drained above.
    let hover: Option<Hover> = request::<HoverRequest>(
        &client,
        2,
        HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position {
                    line: 0,
                    character: 3,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        },
        &mut diagnostics,
    );
    assert!(hover.is_some(), "expected hover over `bad`");
    assert!(
        !diagnostics.is_empty(),
        "the type error should have been published over the transport"
    );

    // Clean shutdown of the production loop.
    let _: () = request::<Shutdown>(&client, 3, (), &mut diagnostics);
    notify::<lsp_types::notification::Exit>(&client, ());
    server.join().unwrap().expect("server loop exited cleanly");
}

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
use lsp_types::request::{
    GotoDefinition, HoverRequest, Initialize, PrepareRenameRequest, References, Rename, Shutdown,
    WorkspaceSymbolRequest,
};
use lsp_types::{
    ClientCapabilities, DidOpenTextDocumentParams, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverParams, InitializeParams, InitializeResult, InitializedParams, Location,
    PartialResultParams, Position, PrepareRenameResponse, PublishDiagnosticsParams,
    ReferenceContext, ReferenceParams, RenameParams, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Uri, WorkDoneProgressParams, WorkspaceEdit, WorkspaceSymbolParams,
    WorkspaceSymbolResponse,
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

    // A malformed notification (garbage params for a known method) must not
    // kill the loop: the server logs and ignores it. Send one, then prove the
    // session is still alive with another hover round-trip (a dead loop would
    // close the channel and panic the next `recv`).
    client
        .sender
        .send(Message::Notification(Notification::new(
            lsp_types::notification::DidChangeTextDocument::METHOD.to_string(),
            serde_json::json!({ "not": "a valid didChange" }),
        )))
        .unwrap();

    let hover_after: Option<Hover> = request::<HoverRequest>(
        &client,
        3,
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
    assert!(
        hover_after.is_some(),
        "server should still serve requests after a malformed notification"
    );

    // Clean shutdown of the production loop.
    let _: () = request::<Shutdown>(&client, 4, (), &mut diagnostics);
    notify::<lsp_types::notification::Exit>(&client, ());
    server.join().unwrap().expect("server loop exited cleanly");
}

/// 0-indexed (line, character) of the `occurrence`-th (0-based) `needle` in
/// `text`.
fn pos_of(text: &str, needle: &str, occurrence: usize) -> Position {
    let mut from = 0;
    let mut byte = 0;
    for _ in 0..=occurrence {
        let idx = text[from..].find(needle).expect("needle present") + from;
        byte = idx;
        from = idx + needle.len();
    }
    let line = text[..byte].matches('\n').count() as u32;
    let col = byte - text[..byte].rfind('\n').map_or(0, |i| i + 1);
    Position {
        line,
        character: col as u32,
    }
}

/// The regression this whole change exists for, pinned over the *production
/// transport*: a package that declares `[build] src = "./"` with `main.ab` at
/// the package root (the root-layout examples ship this shape). The server discovers the
/// package itself and mints its own URIs, while the test sends the editor-shaped
/// URI (no `/./` segment). Before the fix, that spelling mismatch made
/// find-references return `[]`, rename refuse everything, method-goto return
/// null, and workspace-symbol hand back `/./` URIs. Every direct-handler test
/// missed it because the harness mints both sides through one helper.
#[test]
#[allow(deprecated)] // InitializeParams has deprecated fields we default
fn root_layout_navigation_over_transport() {
    const U: &str = "A1B2C3D4-0000-0000-0000-0000000000";
    let source = format!(
        "unique({U}E1) trait Show {{ fn show(self): Number; }}\n\
         unique({U}E2) struct Foo {{ x: Number }}\n\
         impl Show for Foo {{ fn show(self): Number {{ self.x }} }}\n\
         fn helper(f: Foo): Number {{ f.show() }}\n\
         fn run(f: Foo): Number {{ helper(f) }}\n"
    );

    // `[build] src = "./"` with `main.ab` at the root — not under `src/`.
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    std::fs::write(
        root.join("ambient.toml"),
        "[package]\nname = \"strings\"\nversion = \"0.1.0\"\n\n[build]\nsrc = \"./\"\n",
    )
    .unwrap();
    let main = root.join("main.ab");
    std::fs::write(&main, &source).unwrap();
    // The editor-shaped URI: exactly what a client sends — no `/./` segment.
    let uri: Uri = format!("file://{}", main.display()).parse().unwrap();

    let (server_conn, client) = Connection::memory();
    let server = thread::spawn(move || run_server_with_connection(server_conn));

    let mut diagnostics = Vec::new();
    let _: InitializeResult = request::<Initialize>(
        &client,
        1,
        InitializeParams {
            capabilities: ClientCapabilities::default(),
            ..Default::default()
        },
        &mut diagnostics,
    );
    notify::<Initialized>(&client, InitializedParams {});
    notify::<DidOpenTextDocument>(
        &client,
        DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "ambient".to_string(),
                version: 1,
                text: source.clone(),
            },
        },
    );

    let at = |pos: Position| TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        position: pos,
    };

    // References at the `helper` call site: def (line 3) + call (line 4).
    let call = pos_of(&source, "helper", 1);
    let refs: Vec<Location> = request::<References>(
        &client,
        2,
        ReferenceParams {
            text_document_position: at(call),
            context: ReferenceContext {
                include_declaration: true,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        },
        &mut diagnostics,
    )
    .unwrap_or_default();
    let ref_lines: Vec<u32> = refs.iter().map(|l| l.range.start.line).collect();
    assert!(ref_lines.contains(&3), "helper definition: {ref_lines:?}");
    assert!(ref_lines.contains(&4), "helper call site: {ref_lines:?}");
    assert!(
        refs.iter().all(|l| !l.uri.as_str().contains("/./")),
        "reference URIs must be clean: {:?}",
        refs.iter().map(|l| l.uri.as_str()).collect::<Vec<_>>()
    );

    // prepareRename at the same spot returns the identifier range.
    let prep: Option<PrepareRenameResponse> =
        request::<PrepareRenameRequest>(&client, 3, at(call), &mut diagnostics);
    assert!(prep.is_some(), "prepareRename should offer a range");

    // rename returns edits (not an empty/refused response).
    let edit: Option<WorkspaceEdit> = request::<Rename>(
        &client,
        4,
        RenameParams {
            text_document_position: at(call),
            new_name: "helper2".to_string(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        },
        &mut diagnostics,
    );
    let changes = edit
        .expect("rename should return an edit")
        .changes
        .expect("rename should produce changes");
    let total_edits: usize = changes.values().map(Vec::len).sum();
    assert_eq!(total_edits, 2, "rename should rewrite def + call");

    // Goto on the method call `f.show()` in `helper` lands on the impl method
    // (line 2) — this is the occurrence-index fallback that returned null before.
    let show = pos_of(&source, "show", 2);
    let goto: Option<GotoDefinitionResponse> = request::<GotoDefinition>(
        &client,
        5,
        GotoDefinitionParams {
            text_document_position_params: at(show),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        },
        &mut diagnostics,
    );
    match goto {
        Some(GotoDefinitionResponse::Scalar(loc)) => {
            assert_eq!(
                loc.range.start.line, 2,
                "goto should land on the impl method"
            );
            assert!(!loc.uri.as_str().contains("/./"), "goto URI must be clean");
        }
        other => panic!("expected a single goto location, got {other:?}"),
    }

    // workspace/symbol URIs must be clean (no `/./`).
    let syms: Option<WorkspaceSymbolResponse> = request::<WorkspaceSymbolRequest>(
        &client,
        6,
        WorkspaceSymbolParams {
            query: "helper".to_string(),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        },
        &mut diagnostics,
    );
    let Some(WorkspaceSymbolResponse::Flat(flat)) = syms else {
        panic!("expected a flat workspace-symbol response");
    };
    assert!(!flat.is_empty(), "workspace/symbol should find `helper`");
    assert!(
        flat.iter()
            .all(|s| !s.location.uri.as_str().contains("/./")),
        "workspace-symbol URIs must be clean: {:?}",
        flat.iter()
            .map(|s| s.location.uri.as_str())
            .collect::<Vec<_>>()
    );

    let _: () = request::<Shutdown>(&client, 7, (), &mut diagnostics);
    notify::<lsp_types::notification::Exit>(&client, ());
    server.join().unwrap().expect("server loop exited cleanly");
}

//! Semantic-tokens integration test, driven through the real LSP server.
//!
//! Tokens come from the analysed AST of the open document, so a single
//! in-memory file (no package) is enough.

use ambient_lsp::test_harness::TestClient;
use lsp_types::Uri;

#[test]
fn semantic_tokens_are_produced_for_a_document() {
    let mut client = TestClient::new();
    let uri: Uri = "file:///test.ab".parse().unwrap();
    client.open_document(uri.clone(), "fn greet(): number { 42 }\n");

    let tokens = client.semantic_tokens(&uri);

    // A non-trivial document must yield at least one highlighting token (the
    // `fn` keyword, the name, the literal, ...). We don't pin the exact
    // encoding here — just that the full-document request returns tokens.
    assert!(
        !tokens.is_empty(),
        "expected semantic tokens for a non-empty document"
    );

    client.shutdown();
}

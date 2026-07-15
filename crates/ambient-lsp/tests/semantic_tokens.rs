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
    client.open_document(uri.clone(), "fn greet(): Number { 42 }\n");

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

/// Decode delta-encoded tokens into absolute `(line, char, length, type)`.
fn decode(tokens: &[lsp_types::SemanticToken]) -> Vec<(u32, u32, u32, u32)> {
    let mut out = Vec::new();
    let (mut line, mut start) = (0u32, 0u32);
    for t in tokens {
        line += t.delta_line;
        start = if t.delta_line == 0 {
            start + t.delta_start
        } else {
            t.delta_start
        };
        out.push((line, start, t.length, t.token_type));
    }
    out
}

#[test]
fn associated_type_names_get_type_tokens() {
    let source = "unique(A1B2C3D4-0000-0000-0000-000000000001) struct Point { x: Number }\n\
         unique(A1B2C3D4-0000-0000-0000-000000000002) trait Show { type Out; fn show(self): String; }\n\
         impl Show for Point { type Out = String; fn show(self): String { \"p\" } }\n";
    let mut client = TestClient::new();
    let uri: Uri = "file:///test.ab".parse().unwrap();
    client.open_document(uri.clone(), source);

    let decoded = decode(&client.semantic_tokens(&uri));

    // The `Out` name in the trait declaration and the impl binding both get a
    // TYPE (index 3) declaration token.
    for (line_no, line) in source.lines().enumerate() {
        let Some(col) = line.find("type Out") else {
            continue;
        };
        let expected = (
            u32::try_from(line_no).unwrap(),
            u32::try_from(col + "type ".len()).unwrap(),
            3u32, // "Out".len()
            3u32, // TYPE
        );
        assert!(
            decoded.contains(&expected),
            "expected TYPE token {expected:?} on line {line_no}, got: {decoded:?}"
        );
    }

    client.shutdown();
}

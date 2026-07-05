//! Document-symbol integration test, driven through the real LSP server.
//!
//! Symbols are derived from the analysed AST of the open document, so a single
//! in-memory file (no package) is enough.

use ambient_lsp::test_harness::TestClient;
use lsp_types::{SymbolKind, Uri};

#[test]
fn document_symbol_lists_top_level_items() {
    let mut client = TestClient::new();
    let uri: Uri = "file:///test.ab".parse().unwrap();
    client.open_document(
        uri.clone(),
        "fn foo(): Number { 1 }\nfn bar(): Number { foo() }\n",
    );

    let symbols = client.document_symbol(&uri);

    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names.contains(&"foo") && names.contains(&"bar"),
        "expected both functions as document symbols, got: {names:?}"
    );

    // Functions should be reported with FUNCTION kind.
    let foo = symbols.iter().find(|s| s.name == "foo").unwrap();
    assert_eq!(foo.kind, SymbolKind::FUNCTION);

    client.shutdown();
}

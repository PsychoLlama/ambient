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

#[test]
fn trait_and_impl_symbols_include_assoc_types_and_methods() {
    let mut client = TestClient::new();
    let uri: Uri = "file:///test.ab".parse().unwrap();
    client.open_document(
        uri.clone(),
        "unique(A1B2C3D4-0000-0000-0000-000000000001) struct Point { x: Number }\n\
         unique(A1B2C3D4-0000-0000-0000-000000000002) trait Show { type Out; fn show(self): String; }\n\
         impl Show for Point { type Out = String; fn show(self): String { \"p\" } }\n",
    );

    let symbols = client.document_symbol(&uri);

    let trait_sym = symbols.iter().find(|s| s.name == "Show").unwrap();
    let children = trait_sym.children.as_deref().unwrap_or_default();
    let child_names: Vec<&str> = children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        child_names,
        ["Out", "show"],
        "trait children in source order"
    );
    let out = children.iter().find(|c| c.name == "Out").unwrap();
    assert_eq!(out.kind, SymbolKind::TYPE_PARAMETER);

    let impl_sym = symbols
        .iter()
        .find(|s| s.name == "impl Show for Point")
        .unwrap_or_else(|| {
            panic!(
                "expected `impl Show for Point` symbol, got: {:?}",
                symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
            )
        });
    let children = impl_sym.children.as_deref().unwrap_or_default();
    let child_names: Vec<&str> = children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        child_names,
        ["Out", "show"],
        "impl children in source order"
    );
    let out = children.iter().find(|c| c.name == "Out").unwrap();
    assert_eq!(out.detail.as_deref(), Some("= String"));

    client.shutdown();
}

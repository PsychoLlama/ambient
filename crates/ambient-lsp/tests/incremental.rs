//! Warm reanalysis through the server's incremental analysis session.
//!
//! Open → analyze → edit → analyze, driven end-to-end over the LSP transport.
//! The session memoizes per-module checks, but a cross-module signature change
//! must still surface (and later clear) a type error in the importing file —
//! exactly as a cold `ambient check` would report it. `parity.rs` pins the
//! byte-identical equality of the two frontends; this pins that the *warm*
//! path tracks edits correctly rather than replaying stale diagnostics.

use ambient_lsp::test_harness::TestClient;
use lsp_types::Uri;

const MAIN: &str = "use pkg::utils::helper;\npub fn run(): Number { helper() }\n";
const UTILS: &str = "pub fn helper(): Number { 1 }\n";

/// Build an in-memory two-module package and open `main`. Returns the client
/// and both URIs. The whole package lives in the session, so `utils` is
/// analyzed cross-module even though only `main` is opened in the editor.
fn setup() -> (TestClient, Uri, Uri) {
    let mut client = TestClient::with_package("test", &[("main.ab", MAIN), ("utils.ab", UTILS)]);
    let main_uri = client.uri("main.ab");
    let utils_uri = client.uri("utils.ab");
    client.open_document(main_uri.clone(), MAIN);
    (client, main_uri, utils_uri)
}

#[test]
fn cross_module_signature_edit_surfaces_and_clears_in_importer() {
    let (mut client, main_uri, utils_uri) = setup();

    // Clean to start: `main` calls `helper(): Number` and returns Number.
    assert!(
        client.get_diagnostics(&main_uri).is_empty(),
        "clean package: {:?}",
        client.get_diagnostics(&main_uri)
    );

    // Open `utils` and change `helper`'s return type to String. `main` (which
    // uses the result as a Number) must now report a type error — the warm
    // session recomputes `main`'s key because `utils`'s interface hash moved.
    client.open_document(utils_uri.clone(), "pub fn helper(): Number { 1 }\n");
    client.change_document(&utils_uri, "pub fn helper(): String { \"x\" }\n");
    assert!(
        !client.get_diagnostics(&main_uri).is_empty(),
        "importer must see the cross-module signature change"
    );

    // Revert: the error must clear in `main` on the warm path.
    client.change_document(&utils_uri, "pub fn helper(): Number { 1 }\n");
    assert!(
        client.get_diagnostics(&main_uri).is_empty(),
        "importer error must clear after revert: {:?}",
        client.get_diagnostics(&main_uri)
    );

    client.shutdown();
}

#[test]
fn body_only_edit_keeps_importer_clean() {
    let (mut client, main_uri, utils_uri) = setup();

    // A body-only edit to `utils` (interface unchanged) must not perturb
    // `main`'s diagnostics — it stays clean, served from the memo.
    client.open_document(utils_uri.clone(), "pub fn helper(): Number { 1 }\n");
    client.change_document(&utils_uri, "pub fn helper(): Number { 1 + 0 }\n");
    assert!(
        client.get_diagnostics(&main_uri).is_empty(),
        "importer stays clean"
    );
    assert!(
        client.get_diagnostics(&utils_uri).is_empty(),
        "utils stays clean"
    );

    client.shutdown();
}

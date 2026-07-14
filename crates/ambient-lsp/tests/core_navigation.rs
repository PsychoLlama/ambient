//! Goto-definition into embedded core sources.
//!
//! Core/platform modules are compiled into the binary, so a builtin item
//! resolves during checking but has no on-disk file. The server materializes
//! the embedded sources to a content-addressed cache dir (pointed at a temp dir
//! by the harness) and maps builtin modules to `file://` URIs there, so
//! navigation lands in a real (read-only) file. These tests pin the three
//! shapes: a plain core `pub fn`, a core ability method, and an extern fn.

use ambient_lsp::test_harness::{LspTest, TestClient};

/// A package whose `main.ab` references one item of each shape core defines.
/// `boom` performs `Exception::throw` (so it must declare `with Exception`).
const MAIN: &str = "\
fn deep(o: Option<Option<Number>>): Option<Number> {
  core::option::flatten/*flatten*/(o)
}

fn render(x: Number): String {
  core::convert::to_string/*to_string*/(x)
}

fn boom(): Number with Exception {
  Exception::throw/*throw*/!(\"bad\")
}
";

/// A package with `MAIN` as its opened root module.
fn package_test() -> LspTest {
    LspTest::new()
        .with_package()
        .with_file("src/main.ab", MAIN)
        .open_file("src/main.ab")
}

#[test]
fn goto_core_function_lands_in_core_source() {
    // `core::option::flatten` is a plain `pub fn` — the prelude re-exports the
    // `Option` type it operates on; the function lives in `core::option`.
    package_test()
        .goto_definition_at("flatten")
        .expect_file("option.ab")
        .done()
        .shutdown();
}

#[test]
fn goto_extern_fn_lands_in_core_source() {
    // `core::convert::to_string` is a `pub extern fn` declaration.
    package_test()
        .goto_definition_at("to_string")
        .expect_file("convert.ab")
        .done()
        .shutdown();
}

#[test]
fn goto_core_ability_method_lands_in_core_source() {
    // `Exception::throw` perform: served from the occurrence index, which now
    // includes builtin ability-method declarations. Lands in `core::exception`.
    package_test()
        .goto_definition_at("throw")
        .expect_file("exception.ab")
        .done()
        .shutdown();
}

/// Opening a materialized core file (the editor's `didOpen` when the user
/// navigates into one) must publish no diagnostics: it is a read-only view of a
/// builtin already checked in place, and standalone analysis of core's
/// `unique(...)`/`extern fn`/self-import shapes would otherwise be noisy.
#[test]
fn opening_a_materialized_core_file_publishes_no_diagnostics() {
    // A clean source (no cursor markers) so `pos_of` lands on the call exactly.
    const SRC: &str = "fn render(x: Number): String {\n  core::convert::to_string(x)\n}\n";
    let mut client = TestClient::with_package("test", &[("main.ab", SRC)]);
    let main_uri = client.uri("main.ab");
    client.open_document(main_uri.clone(), SRC);

    // Navigate into the core extern fn to discover its materialized URI.
    let (line, ch) = pos_of(SRC, "to_string", 0);
    let locations = client.goto_definition(&main_uri, line, ch);
    assert!(!locations.is_empty(), "goto into core produced no location");
    let core_uri = locations[0].uri.clone();
    assert!(
        core_uri.as_str().ends_with("convert.ab"),
        "expected a core file uri, got {}",
        core_uri.as_str()
    );

    // The editor now opens that file. Read the on-disk (materialized) content
    // and feed it through `didOpen`, exactly as an editor would.
    let path = core_uri.as_str().strip_prefix("file://").expect("file uri");
    let content = std::fs::read_to_string(path).expect("materialized core file readable");
    client.open_document(core_uri.clone(), &content);

    let diagnostics = client.get_diagnostics(&core_uri);
    assert!(
        diagnostics.is_empty(),
        "materialized core file produced diagnostics: {diagnostics:?}"
    );
    client.shutdown();
}

/// Byte position `(line, character)` of the `occurrence`-th `needle` in `text`.
fn pos_of(text: &str, needle: &str, occurrence: usize) -> (u32, u32) {
    let mut from = 0;
    let mut byte = 0;
    for _ in 0..=occurrence {
        let idx = text[from..].find(needle).expect("needle") + from;
        byte = idx;
        from = idx + needle.len();
    }
    let line = text[..byte].matches('\n').count() as u32;
    let col = byte - text[..byte].rfind('\n').map_or(0, |i| i + 1);
    (line, col as u32)
}

//! Goto-definition into embedded core sources.
//!
//! Core/platform modules are compiled into the binary, so a builtin item
//! resolves during checking but has no on-disk file. The server materializes
//! the embedded sources to a content-addressed cache dir (pointed at a temp dir
//! by the harness) and maps builtin modules to `file://` URIs there, so
//! navigation lands in a real (read-only) file. These tests pin the three
//! shapes: a plain core `pub fn`, a core ability method, and an extern fn.

use ambient_lsp::test_harness::{LspTest, TestClient};
use lsp_types::Location;

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

/// A package exercising builtin *method* navigation: an associated/inherent
/// call (`String::join`) and a dot-dispatch call (`"a".concat(..)`). Both
/// dispatch to core impl methods on `String`, whose declarations only carry a
/// navigable `Method` occurrence once the builtin module is *checked* — the
/// registry's parse+resolve-only builtin ASTs lack the impl-method dispatch
/// symbol. The session memoizes that check; this pins that the declaration side
/// now lands in the materialized core source, like plain items and ability
/// methods already do.
const METHODS: &str = "\
fn joined(parts: List<String>): String {
  String::join/*join*/(parts, \", \")
}

fn glued(): String {
  \"a\".concat/*concat*/(\"b\")
}
";

/// A package with `METHODS` as its opened root module.
fn methods_test() -> LspTest {
    LspTest::new()
        .with_package()
        .with_file("src/main.ab", METHODS)
        .open_file("src/main.ab")
}

/// Assert the single definition location lands in `filename` with its range
/// covering exactly `method` — the method's *name* span inside the materialized
/// core source, read back off disk to confirm the byte range is the declaration.
fn assert_method_declaration(locations: &[Location], filename: &str, method: &str) {
    let loc = locations
        .iter()
        .find(|l| l.uri.as_str().ends_with(filename))
        .unwrap_or_else(|| panic!("no definition in {filename}; got {locations:?}"));
    let path = loc
        .uri
        .as_str()
        .strip_prefix("file://")
        .expect("file uri into the materialized cache");
    // A materialized read-only cache copy, not the package's own `src/` tree.
    assert!(
        !path.contains("/src/"),
        "expected a materialized core-cache path, not a package source, got {path}"
    );
    let content = std::fs::read_to_string(path).expect("materialized core file readable");
    let line = content
        .lines()
        .nth(loc.range.start.line as usize)
        .expect("declaration line present");
    let (start, end) = (
        loc.range.start.character as usize,
        loc.range.end.character as usize,
    );
    assert_eq!(
        &line[start..end],
        method,
        "definition range should cover the method name in {filename}"
    );
}

#[test]
fn goto_builtin_associated_method_lands_in_core_source() {
    // `String::join(..)`: an associated call the checker rewrites to the impl
    // method's dispatch symbol. Navigation now reaches the checked declaration.
    let (test, locations) = methods_test().goto_definition_at("join").raw();
    assert_method_declaration(&locations, "string.ab", "join");
    test.shutdown();
}

#[test]
fn goto_builtin_dot_method_lands_in_core_source() {
    // `"a".concat("b")`: dot dispatch on a `String` receiver, resolved to the
    // same kind of impl-method symbol. Navigation lands on its declaration.
    let (test, locations) = methods_test().goto_definition_at("concat").raw();
    assert_method_declaration(&locations, "string.ab", "concat");
    test.shutdown();
}

/// A package using the bare, prelude-injected enum-variant constructors and
/// patterns (`Some`/`Ok`), which resolve with no `use` and no `core::` prefix.
const VARIANTS: &str = "\
fn make(): Option<Number> {
  Some/*some_expr*/(1)
}

fn take(o: Option<Number>): Number {
  match o {
    Some/*some_pat*/(n) => n,
    None => 0
  }
}

fn wrap(): Result<Number, Number> {
  Ok/*ok_expr*/(1)
}
";

/// A package with `VARIANTS` as its opened root module.
fn variants_test() -> LspTest {
    LspTest::new()
        .with_package()
        .with_file("src/main.ab", VARIANTS)
        .open_file("src/main.ab")
}

#[test]
fn goto_bare_prelude_variant_some_expr_lands_in_core() {
    // Bare `Some(..)` is a prelude re-export of `core::option::Some` — no `use`,
    // no prefix. The engine's resolve pass canonicalizes it to the `[Option,
    // Some]` variant identity; navigation consumes that (the prelude excludes
    // variants from `resolve_imports`, so the spelling-based walk can't).
    variants_test()
        .goto_definition_at("some_expr")
        .expect_file("option.ab")
        .done()
        .shutdown();
}

#[test]
fn goto_bare_prelude_variant_some_pattern_lands_in_core() {
    // A bare variant *pattern* lands on the same `[Option, Some]` identity as
    // the constructor, so it navigates to the same core declaration.
    variants_test()
        .goto_definition_at("some_pat")
        .expect_file("option.ab")
        .done()
        .shutdown();
}

#[test]
fn goto_bare_prelude_variant_ok_lands_in_core() {
    // `Ok` rides the prelude from `core::result`, distinct from `Option`.
    variants_test()
        .goto_definition_at("ok_expr")
        .expect_file("result.ab")
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

//! Find-references integration tests, driven through the real LSP server.
//!
//! References resolve the symbol under the cursor via the module registry,
//! then query the on-disk `SymbolDb` (`build/symbols.db`) for the symbol's
//! dependents. That database is populated once, by a full package compile on
//! the first `didOpen`, and is *never* refreshed afterward (a documented
//! staleness gap; see the stale-after-edit test at the bottom). These tests
//! therefore always operate on real temp packages so the compile has files to
//! read.
//!
//! Note on granularity: a "reference" is reported at the *name span of the
//! calling function's definition*, not at the individual call expression. That
//! is the current behavior of `handle_references`; the tests pin it rather than
//! assume a finer resolution.

use std::fs;
use std::path::Path;

use ambient_lsp::test_harness::TestClient;
use lsp_types::Uri;
use tempfile::TempDir;

/// A real on-disk temp package plus a live server client.
struct Fixture {
    _temp: TempDir,
    client: TestClient,
    root: std::path::PathBuf,
}

impl Fixture {
    /// Write an `ambient.toml` + the given `src/*` files, spawn a server, and
    /// open `main_file`. The package compiles on first `didOpen`, populating
    /// the symbol database that references reads.
    fn new(files: &[(&str, &str)], main_file: &str) -> Self {
        let temp = TempDir::new().expect("create temp dir");
        let root = temp.path().to_path_buf();

        let manifest =
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n\n[build]\nsrc = \"src\"\n";
        fs::write(root.join("ambient.toml"), manifest).expect("write manifest");

        for (path, content) in files {
            let full = root.join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).expect("create src dir");
            }
            fs::write(&full, content).expect("write source file");
        }

        let mut client = TestClient::new();
        let main = &root.join(main_file);
        client.open_document(uri_for(main), fs::read_to_string(main).unwrap().as_str());

        Fixture {
            _temp: temp,
            client,
            root,
        }
    }

    /// The `file://` URI for a package-relative path.
    fn uri(&self, rel: &str) -> Uri {
        uri_for(&self.root.join(rel))
    }
}

/// Convert a filesystem path to a `file://` URI (matches the harness builder).
fn uri_for(path: &Path) -> Uri {
    format!("file://{}", path.to_string_lossy())
        .parse()
        .expect("parse uri")
}

/// 0-indexed (line, character) of the `occurrence`-th (0-based) appearance of
/// `needle` in `text`. Sources here are ASCII, so byte offset == UTF-16 column.
fn pos_of(text: &str, needle: &str, occurrence: usize) -> (u32, u32) {
    let mut search_from = 0;
    let mut byte = None;
    for _ in 0..=occurrence {
        let idx = text[search_from..]
            .find(needle)
            .map(|i| search_from + i)
            .expect("needle not found");
        byte = Some(idx);
        search_from = idx + needle.len();
    }
    let byte = byte.unwrap();
    let line = text[..byte].matches('\n').count() as u32;
    let line_start = text[..byte].rfind('\n').map_or(0, |i| i + 1);
    let character = (byte - line_start) as u32;
    (line, character)
}

const UTILS: &str = "pub fn target(): number { 42 }\n";

/// A caller in a separate module that imports and calls `target`.
const MAIN_ONE_CALLER: &str = "\
use pkg::utils::{target};
fn caller(): number { target() }
";

#[test]
fn references_finds_cross_module_caller() {
    let mut fx = Fixture::new(
        &[("src/utils.ab", UTILS), ("src/main.ab", MAIN_ONE_CALLER)],
        "src/main.ab",
    );

    // Cursor on the `target` call site in main.ab (the second `target`; the
    // first is inside the `use` import).
    let (line, ch) = pos_of(MAIN_ONE_CALLER, "target", 1);
    let uri = fx.uri("src/main.ab");
    let refs = fx.client.references(&uri, line, ch, false);

    assert!(
        !refs.is_empty(),
        "expected the cross-module caller to be reported"
    );
    assert!(
        refs.iter().any(|l| l.uri.as_str().ends_with("main.ab")),
        "expected a reference in main.ab (the caller), got: {:?}",
        refs.iter().map(|l| l.uri.as_str()).collect::<Vec<_>>()
    );

    fx.client.shutdown();
}

#[test]
fn references_include_declaration_adds_the_definition() {
    let mut fx = Fixture::new(
        &[("src/utils.ab", UTILS), ("src/main.ab", MAIN_ONE_CALLER)],
        "src/main.ab",
    );

    let (line, ch) = pos_of(MAIN_ONE_CALLER, "target", 1);
    let uri = fx.uri("src/main.ab");

    let without = fx.client.references(&uri, line, ch, false);
    let with = fx.client.references(&uri, line, ch, true);

    // Including the declaration adds exactly the definition (in utils.ab) on
    // top of the dependents.
    assert_eq!(
        with.len(),
        without.len() + 1,
        "include_declaration should add the definition location.\n  without: {:?}\n  with: {:?}",
        without,
        with
    );
    assert!(
        with.iter().any(|l| l.uri.as_str().ends_with("utils.ab")),
        "expected the declaration (utils.ab) among the results, got: {:?}",
        with.iter().map(|l| l.uri.as_str()).collect::<Vec<_>>()
    );
    assert!(
        !without.iter().any(|l| l.uri.as_str().ends_with("utils.ab")),
        "declaration should be absent when include_declaration is false"
    );

    fx.client.shutdown();
}

#[test]
fn references_on_uncalled_symbol_is_empty() {
    // `orphan` resolves to a real symbol, but nothing *depends* on it, so the
    // dependents query comes back empty and the handler returns an empty list
    // without erroring.
    //
    // Getting a resolvable-but-uncalled symbol is deliberately awkward: the
    // symbol db only records call edges between functions/lambdas, and in
    // clean code every function-name reference sits inside some function that
    // then becomes a dependent. So we reference `orphan` from a `const`
    // initializer instead. The checker rejects that (consts must be literal),
    // but the recovering parser the IDE uses still yields a resolvable
    // `orphan()` call under the cursor — and because a const is neither a
    // function nor a lambda, no dependent edge is recorded for it.
    let utils = "pub fn orphan(): number { 7 }\n";
    let main = "\
use pkg::utils::{orphan};
const HELD: number = orphan();
";
    let mut fx = Fixture::new(
        &[("src/utils.ab", utils), ("src/main.ab", main)],
        "src/main.ab",
    );

    // Cursor on the `orphan` reference in the const (occurrence 1; occurrence 0
    // is the `use` import, which is not an expression).
    let uri = fx.uri("src/main.ab");
    let (line, ch) = pos_of(main, "orphan", 1);
    let refs = fx.client.references(&uri, line, ch, false);

    assert!(
        refs.is_empty(),
        "expected no references for a symbol with no calling functions, got: {:?}",
        refs
    );

    fx.client.shutdown();
}

#[test]
fn references_on_non_name_expression_is_empty() {
    // Cursor on a numeric literal resolves to no name; the handler returns an
    // empty list rather than erroring.
    let main = "fn main(): number { 42 }\n";
    let mut fx = Fixture::new(&[("src/main.ab", main)], "src/main.ab");

    let uri = fx.uri("src/main.ab");
    let (line, ch) = pos_of(main, "42", 0);
    let refs = fx.client.references(&uri, line, ch, false);

    assert!(refs.is_empty(), "expected empty, got: {:?}", refs);

    fx.client.shutdown();
}

#[test]
fn references_are_stale_after_edit_adding_a_caller() {
    // PINS A KNOWN WART. The symbol database is snapshotted once, at the first
    // `didOpen`, and never refreshed. An edit that adds a new caller is NOT
    // reflected in find-references results.
    //
    // Follow-up 3 will make the database refresh on edits; when it does, this
    // assertion should flip (the new caller SHOULD be found). Update this test
    // deliberately at that point rather than deleting it silently.
    let main_before = MAIN_ONE_CALLER;
    let mut fx = Fixture::new(
        &[("src/utils.ab", UTILS), ("src/main.ab", main_before)],
        "src/main.ab",
    );

    let uri = fx.uri("src/main.ab");
    let (line, ch) = pos_of(main_before, "target", 1);
    let before = fx.client.references(&uri, line, ch, false);
    assert!(
        !before.is_empty(),
        "precondition: the original caller must be found before the edit"
    );

    // Add a second caller of `target` via an edit.
    let main_after = "\
use pkg::utils::{target};
fn caller(): number { target() }
fn caller_two(): number { target() }
";
    fx.client.change_document(&uri, main_after);

    let after = fx.client.references(&uri, line, ch, false);

    assert_eq!(
        after.len(),
        before.len(),
        "STALE-BY-DESIGN: the edit-added caller must NOT appear until follow-up 3 \
         refreshes the symbol db. before: {:?}, after: {:?}",
        before,
        after
    );

    fx.client.shutdown();
}

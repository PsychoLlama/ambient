//! Find-references integration tests, driven through the real LSP server.
//!
//! References are served from the occurrence index owned by `ambient-analysis`
//! (`collect_occurrences`): every definition and reference site of every
//! symbol, with exact spans, for every module in the package. The index is
//! rebuilt on every edit alongside the module registry, so results are always
//! fresh — including references in files that were never opened in the editor.
//!
//! Ranges are exact: a "reference" is the identifier at the use site (the
//! `target` in `target()`), not the whole enclosing function. These tests
//! operate on real temp packages so the whole package is on disk to index.

use ambient_lsp::test_harness::TestClient;
use lsp_types::{Location, Uri};

/// An in-memory package plus a live server client.
struct Fixture {
    client: TestClient,
}

impl Fixture {
    /// Build an in-memory package from the given `src/*` files and open
    /// `main_file`. The whole package lives in the session, so every module is
    /// indexed for references (opened or not).
    fn new(files: &[(&str, &str)], main_file: &str) -> Self {
        let pkg: Vec<(&str, &str)> = files.iter().map(|(p, c)| (strip_src(p), *c)).collect();
        let mut client = TestClient::with_package("test", &pkg);

        let main_content = files
            .iter()
            .find(|(p, _)| *p == main_file)
            .expect("main file among files")
            .1;
        client.open_document(client.uri(strip_src(main_file)), main_content);

        Fixture { client }
    }

    /// The `file://` URI for a package-relative path (`src/utils.ab`).
    fn uri(&self, rel: &str) -> Uri {
        self.client.uri(strip_src(rel))
    }
}

/// A `src/`-prefixed test path as the `src`-relative module path the in-memory
/// package addresses modules by.
fn strip_src(path: &str) -> &str {
    path.strip_prefix("src/").unwrap_or(path)
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

/// The number of characters a location's range spans on a single line.
fn range_width(loc: &Location) -> u32 {
    assert_eq!(
        loc.range.start.line, loc.range.end.line,
        "expected a single-line range"
    );
    loc.range.end.character - loc.range.start.character
}

const UTILS: &str = "pub fn target(): Number { 42 }\n";

/// A caller in a separate module that imports and calls `target`.
const MAIN_ONE_CALLER: &str = "\
use pkg::utils::{target};
fn caller(): Number { target() }
";

#[test]
fn references_finds_cross_module_caller() {
    let mut fx = Fixture::new(
        &[("src/utils.ab", UTILS), ("src/main.ab", MAIN_ONE_CALLER)],
        "src/main.ab",
    );

    // Cursor on the `target` call site in main.ab (occurrence 1; occurrence 0
    // is the `target` inside the `use` import).
    let (line, ch) = pos_of(MAIN_ONE_CALLER, "target", 1);
    let uri = fx.uri("src/main.ab");
    let refs = fx.client.references(&uri, line, ch, false);

    assert!(
        refs.iter().any(|l| l.uri.as_str().ends_with("main.ab")),
        "expected a reference in main.ab (the caller), got: {:?}",
        refs.iter().map(|l| l.uri.as_str()).collect::<Vec<_>>()
    );

    fx.client.shutdown();
}

#[test]
fn references_have_exact_identifier_ranges() {
    // A reference range covers exactly the `target` identifier (6 chars), not
    // the whole calling function — the core improvement over the old
    // whole-function-span behavior.
    let mut fx = Fixture::new(
        &[("src/utils.ab", UTILS), ("src/main.ab", MAIN_ONE_CALLER)],
        "src/main.ab",
    );

    let (line, ch) = pos_of(MAIN_ONE_CALLER, "target", 1);
    let uri = fx.uri("src/main.ab");
    let refs = fx.client.references(&uri, line, ch, true);

    assert!(!refs.is_empty());
    for loc in &refs {
        assert_eq!(
            range_width(loc),
            "target".len() as u32,
            "every range should cover exactly the identifier: {loc:?}"
        );
    }

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

    // Including the declaration adds exactly the definition (in utils.ab).
    assert_eq!(
        with.len(),
        without.len() + 1,
        "include_declaration should add the definition location.\n  without: {without:?}\n  with: {with:?}"
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
fn references_reach_a_caller_in_an_unopened_file() {
    // `other.ab` is never opened, yet its call of `target` must be found: the
    // occurrence index walks every module the package discovered on disk.
    let other = "\
use pkg::utils::{target};
fn other_caller(): Number { target() + 1 }
";
    let mut fx = Fixture::new(
        &[
            ("src/utils.ab", UTILS),
            ("src/main.ab", MAIN_ONE_CALLER),
            ("src/other.ab", other),
        ],
        "src/main.ab",
    );

    let (line, ch) = pos_of(MAIN_ONE_CALLER, "target", 1);
    let uri = fx.uri("src/main.ab");
    let refs = fx.client.references(&uri, line, ch, false);

    assert!(
        refs.iter().any(|l| l.uri.as_str().ends_with("other.ab")),
        "expected a reference in the never-opened other.ab, got: {:?}",
        refs.iter().map(|l| l.uri.as_str()).collect::<Vec<_>>()
    );

    fx.client.shutdown();
}

#[test]
fn references_on_never_referenced_symbol_is_only_its_declaration() {
    // `lonely` is defined but imported and called nowhere, so without the
    // declaration there are no references at all.
    let utils = "pub fn lonely(): Number { 7 }\n";
    let main = "pub fn run(): Number { 1 }\n";
    let mut fx = Fixture::new(
        &[("src/utils.ab", utils), ("src/main.ab", main)],
        "src/utils.ab",
    );

    // Cursor on the definition of `lonely` in utils.ab.
    let uri = fx.uri("src/utils.ab");
    let (line, ch) = pos_of(utils, "lonely", 0);

    let without = fx.client.references(&uri, line, ch, false);
    assert!(
        without.is_empty(),
        "a symbol referenced nowhere has no references, got: {without:?}"
    );

    let with = fx.client.references(&uri, line, ch, true);
    assert_eq!(with.len(), 1, "only the declaration itself: {with:?}");

    fx.client.shutdown();
}

#[test]
fn references_on_non_name_expression_is_empty() {
    // Cursor on a numeric literal is on no occurrence; empty, no error.
    let main = "fn main(): Number { 42 }\n";
    let mut fx = Fixture::new(&[("src/main.ab", main)], "src/main.ab");

    let uri = fx.uri("src/main.ab");
    let (line, ch) = pos_of(main, "42", 0);
    let refs = fx.client.references(&uri, line, ch, false);

    assert!(refs.is_empty(), "expected empty, got: {refs:?}");

    fx.client.shutdown();
}

#[test]
fn references_are_fresh_after_edit_adding_a_caller() {
    // The occurrence index is rebuilt on every edit, so a newly added caller
    // shows up immediately — the behavior this used to pin as stale-by-design.
    let mut fx = Fixture::new(
        &[("src/utils.ab", UTILS), ("src/main.ab", MAIN_ONE_CALLER)],
        "src/main.ab",
    );

    let uri = fx.uri("src/main.ab");
    let (line, ch) = pos_of(MAIN_ONE_CALLER, "target", 1);
    let before = fx.client.references(&uri, line, ch, false);
    assert!(
        !before.is_empty(),
        "precondition: the original caller must be found before the edit"
    );

    // Add a second caller of `target` via an edit.
    let main_after = "\
use pkg::utils::{target};
fn caller(): Number { target() }
fn caller_two(): Number { target() }
";
    fx.client.change_document(&uri, main_after);

    let after = fx.client.references(&uri, line, ch, false);

    assert_eq!(
        after.len(),
        before.len() + 1,
        "the edit-added caller must appear immediately.\n  before: {before:?}\n  after: {after:?}"
    );
    // The new caller sits on line 2 (0-indexed).
    assert!(
        after.iter().any(|l| l.range.start.line == 2),
        "expected the new caller on line 2, got: {after:?}"
    );

    fx.client.shutdown();
}

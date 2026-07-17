//! Rename integration tests, driven through the LSP request handlers.
//!
//! Rename edits come from the same occurrence index as find-references, so a
//! rename rewrites every definition, reference, and import of a symbol. These
//! tests apply the returned `WorkspaceEdit` to an in-memory shadow of the
//! sources and then re-analyze that shadow with `ambient-analysis` to prove the
//! rename round-trips to a still-clean package.

use std::collections::HashMap;
use std::path::PathBuf;

use ambient_analysis::package::AnalysisPackage;
use ambient_lsp::test_harness::TestClient;
use lsp_types::{PrepareRenameResponse, TextEdit, Uri, WorkspaceEdit};

/// An in-memory package client plus a shadow copy of each module's current
/// source (keyed by `src`-relative path). Rename edits are applied to the
/// shadow, then the shadow is re-analyzed to prove the rename round-trips to a
/// still-clean package — all without touching disk.
struct Fixture {
    client: TestClient,
    sources: HashMap<String, String>,
}

impl Fixture {
    fn new(files: &[(&str, &str)], main_file: &str) -> Self {
        let sources: HashMap<String, String> = files
            .iter()
            .map(|(p, c)| (strip_src(p).to_string(), (*c).to_string()))
            .collect();
        let pkg: Vec<(&str, &str)> = files.iter().map(|(p, c)| (strip_src(p), *c)).collect();
        let mut client = TestClient::with_package("test", &pkg);

        let main_rel = strip_src(main_file);
        let main_content = sources[main_rel].clone();
        client.open_document(client.uri(main_rel), &main_content);

        Fixture { client, sources }
    }

    fn uri(&self, rel: &str) -> Uri {
        self.client.uri(strip_src(rel))
    }

    fn read(&self, rel: &str) -> String {
        self.sources[strip_src(rel)].clone()
    }

    /// The `src`-relative module path a workspace-edit uri refers to.
    fn rel_of_uri(&self, uri: &Uri) -> String {
        self.sources
            .keys()
            .find(|rel| self.uri(rel).as_str() == uri.as_str())
            .cloned()
            .expect("edit uri maps to a known module")
    }

    /// Apply a `WorkspaceEdit` to the shadow sources.
    #[allow(clippy::mutable_key_type)]
    fn apply_workspace_edit(&mut self, edit: &WorkspaceEdit) {
        let changes = edit.changes.as_ref().expect("changes present");
        let edits: Vec<(String, Vec<TextEdit>)> = changes
            .iter()
            .map(|(uri, edits)| (self.rel_of_uri(uri), edits.clone()))
            .collect();
        for (rel, edits) in edits {
            let content = self.sources[&rel].clone();
            self.sources.insert(rel, apply_edits(&content, &edits));
        }
    }

    /// Rebuild an in-memory package from the shadow sources and assert every
    /// module type-checks clean — the round-trip check after a rename.
    fn assert_package_clean(&self) {
        let mut package = AnalysisPackage::empty(
            PathBuf::from("/rename-test"),
            PathBuf::from("/rename-test/src"),
            "test",
        );
        for (rel, content) in &self.sources {
            package.insert_module_at_path(rel, content.clone());
        }
        for (path, result) in package.analyze_all() {
            assert!(
                result.diagnostics().is_empty(),
                "unexpected diagnostics in {path}: {:?}",
                result.diagnostics()
            );
        }
    }
}

/// A `src/`-prefixed test path as the `src`-relative module path.
fn strip_src(path: &str) -> &str {
    path.strip_prefix("src/").unwrap_or(path)
}

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

/// Byte offset of an (ASCII) line/character position.
fn offset_of(content: &str, line: u32, character: u32) -> usize {
    let mut offset = 0usize;
    for (i, l) in content.split_inclusive('\n').enumerate() {
        if i as u32 == line {
            return offset + character as usize;
        }
        offset += l.len();
    }
    offset + character as usize
}

/// Apply a set of single-line text edits to a source string.
fn apply_edits(content: &str, edits: &[TextEdit]) -> String {
    let mut byte_edits: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| {
            let start = offset_of(content, e.range.start.line, e.range.start.character);
            let end = offset_of(content, e.range.end.line, e.range.end.character);
            (start, end, e.new_text.as_str())
        })
        .collect();
    // Apply back-to-front so earlier edits don't shift later offsets.
    byte_edits.sort_by_key(|b| std::cmp::Reverse(b.0));
    let mut out = content.to_string();
    for (start, end, text) in byte_edits {
        out.replace_range(start..end, text);
    }
    out
}

const UTILS: &str = "pub fn target(): Number { 42 }\n";
const MAIN_ONE_CALLER: &str = "\
use pkg::utils::{target};
fn caller(): Number { target() }
";

#[test]
fn rename_function_across_modules_roundtrips() {
    let mut fx = Fixture::new(
        &[("src/utils.ab", UTILS), ("src/main.ab", MAIN_ONE_CALLER)],
        "src/main.ab",
    );

    // Rename from the call site in main.ab.
    let uri = fx.uri("src/main.ab");
    let (line, ch) = pos_of(MAIN_ONE_CALLER, "target", 1);
    let edit = fx
        .client
        .rename(&uri, line, ch, "goal")
        .expect("rename produced an edit");
    fx.apply_workspace_edit(&edit);

    let utils = fx.read("src/utils.ab");
    let main = fx.read("src/main.ab");
    assert!(utils.contains("fn goal"), "definition renamed: {utils:?}");
    assert!(
        !utils.contains("target"),
        "old name gone from utils: {utils:?}"
    );
    assert!(
        main.contains("use pkg::utils::{goal}"),
        "import renamed: {main:?}"
    );
    assert!(main.contains("goal()"), "call renamed: {main:?}");
    assert!(
        !main.contains("target"),
        "old name gone from main: {main:?}"
    );

    fx.assert_package_clean();
    fx.client.shutdown();
}

#[test]
fn rename_local_parameter_same_file_roundtrips() {
    let src = "fn run(count: Number): Number { count + count }\n";
    let mut fx = Fixture::new(&[("src/main.ab", src)], "src/main.ab");

    let uri = fx.uri("src/main.ab");
    // Rename from the param definition.
    let (line, ch) = pos_of(src, "count", 0);
    let edit = fx
        .client
        .rename(&uri, line, ch, "total")
        .expect("rename produced an edit");
    fx.apply_workspace_edit(&edit);

    let main = fx.read("src/main.ab");
    assert_eq!(
        main, "fn run(total: Number): Number { total + total }\n",
        "all three occurrences renamed"
    );

    fx.assert_package_clean();
    fx.client.shutdown();
}

#[test]
fn rename_rejects_a_module_level_collision() {
    // `goal` already exists in utils, so renaming `target` to it is rejected.
    let utils = "pub fn target(): Number { 1 }\npub fn goal(): Number { 2 }\n";
    let mut fx = Fixture::new(
        &[("src/utils.ab", utils), ("src/main.ab", MAIN_ONE_CALLER)],
        "src/main.ab",
    );

    let uri = fx.uri("src/main.ab");
    let (line, ch) = pos_of(MAIN_ONE_CALLER, "target", 1);
    let result = fx.client.try_rename(&uri, line, ch, "goal");
    assert!(
        result.is_err(),
        "rename onto an existing symbol must be rejected, got: {result:?}"
    );

    fx.client.shutdown();
}

#[test]
fn rename_rejects_a_local_collision() {
    let src = "fn run(x: Number, y: Number): Number { x + y }\n";
    let mut fx = Fixture::new(&[("src/main.ab", src)], "src/main.ab");

    let uri = fx.uri("src/main.ab");
    let (line, ch) = pos_of(src, "x", 0);
    let result = fx.client.try_rename(&uri, line, ch, "y");
    assert!(
        result.is_err(),
        "renaming a param onto another binding must be rejected, got: {result:?}"
    );

    fx.client.shutdown();
}

#[test]
fn prepare_rename_allows_a_function_and_rejects_a_literal() {
    let main = "fn helper(): Number { 1 }\nfn run(): Number { helper() + 42 }\n";
    let mut fx = Fixture::new(&[("src/main.ab", main)], "src/main.ab");
    let uri = fx.uri("src/main.ab");

    // On the `helper` definition: renameable, returns its identifier range.
    let (line, ch) = pos_of(main, "helper", 0);
    let allowed = fx.client.prepare_rename(&uri, line, ch);
    match allowed {
        Some(PrepareRenameResponse::Range(range)) => {
            assert_eq!(range.end.character - range.start.character, 6);
        }
        other => panic!("expected a rename range on a function, got: {other:?}"),
    }

    // On the numeric literal `42`: not renameable.
    let (line, ch) = pos_of(main, "42", 0);
    assert!(
        fx.client.prepare_rename(&uri, line, ch).is_none(),
        "a literal is not renameable"
    );

    fx.client.shutdown();
}

#[test]
fn prepare_rename_rejects_a_type() {
    // Types aren't fully indexed yet, so rename is gated off for them.
    let main = "\
unique(A1B2C3D4-0000-0000-0000-000000000001) struct Money { cents: Number }
fn run(): Money { Money { cents: 1 } }
";
    let mut fx = Fixture::new(&[("src/main.ab", main)], "src/main.ab");
    let uri = fx.uri("src/main.ab");

    // Cursor on the `Money` type definition name.
    let (line, ch) = pos_of(main, "Money", 0);
    assert!(
        fx.client.prepare_rename(&uri, line, ch).is_none(),
        "type rename is gated off"
    );

    fx.client.shutdown();
}

const SHAPES: &str = "\
pub unique(A1B2C3D4-0000-0000-0000-0000000000C1) enum Shape { Circle(Number), Square }
fn mk(): Shape { Circle(2.0) }
fn area(s: Shape): Number { match s { Circle(n) => n, Square => 0 } }
";

// The consumer module reaches the variant fully-qualified — as a constructor
// and as a match pattern — with no `use` of the variant itself. Now that
// foreign qualified variants resolve by `Fqn` identity, the post-rename
// package type-checks clean, so the round-trip can span modules.
const SHAPES_CONSUMER: &str = "\
use pkg::shapes;
fn make(): shapes::Shape { shapes::Circle(5.0) }
fn describe(s: shapes::Shape): Number { match s { shapes::Circle(n) => n, shapes::Square => 0 } }
";

#[test]
fn rename_enum_variant_roundtrips_across_modules() {
    // Renaming a variant rewrites its declaration, every constructor, and every
    // pattern — bare (same-module) and fully-qualified (`shapes::Circle` in
    // another module) alike — but never the enum, whose identity is distinct.
    let mut fx = Fixture::new(
        &[("src/shapes.ab", SHAPES), ("src/main.ab", SHAPES_CONSUMER)],
        "src/main.ab",
    );

    // Rename `Circle` from the qualified `shapes::Circle(5.0)` construction
    // site in the *consumer* module.
    let uri = fx.uri("src/main.ab");
    let (line, ch) = pos_of(SHAPES_CONSUMER, "Circle", 0);
    let edit = fx
        .client
        .rename(&uri, line, ch, "Round")
        .expect("variant rename produced an edit");
    fx.apply_workspace_edit(&edit);

    let shapes = fx.read("src/shapes.ab");
    let main = fx.read("src/main.ab");
    assert!(
        shapes.contains("Round(Number)"),
        "declaration renamed: {shapes:?}"
    );
    assert!(
        shapes.contains("Round(2.0)"),
        "constructor renamed: {shapes:?}"
    );
    assert!(shapes.contains("Round(n)"), "pattern renamed: {shapes:?}");
    assert!(
        shapes.contains("enum Shape"),
        "the enum name is untouched: {shapes:?}"
    );
    assert!(
        !shapes.contains("Circle"),
        "old name gone from shapes: {shapes:?}"
    );

    assert!(
        main.contains("shapes::Round(5.0)"),
        "qualified constructor renamed: {main:?}"
    );
    assert!(
        main.contains("shapes::Round(n)"),
        "qualified pattern renamed: {main:?}"
    );
    assert!(
        !main.contains("Circle"),
        "old name gone from main: {main:?}"
    );

    fx.assert_package_clean();
    fx.client.shutdown();
}

#[test]
fn prepare_rename_allows_a_variant_rejects_the_enum() {
    let mut fx = Fixture::new(&[("src/main.ab", SHAPES)], "src/main.ab");
    let shapes_uri = fx.uri("src/main.ab");

    // On the `Circle` variant declaration: renameable.
    let (line, ch) = pos_of(SHAPES, "Circle", 0);
    match fx.client.prepare_rename(&shapes_uri, line, ch) {
        Some(PrepareRenameResponse::Range(range)) => {
            assert_eq!(range.end.character - range.start.character, 6, "`Circle`");
        }
        other => panic!("expected a rename range on a variant, got: {other:?}"),
    }

    // On the `Shape` enum name: gated off (its references aren't fully indexed).
    let (line, ch) = pos_of(SHAPES, "Shape", 0);
    assert!(
        fx.client.prepare_rename(&shapes_uri, line, ch).is_none(),
        "enum rename is gated off"
    );

    fx.client.shutdown();
}

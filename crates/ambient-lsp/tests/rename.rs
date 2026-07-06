//! Rename integration tests, driven through the real LSP server.
//!
//! Rename edits come from the same occurrence index as find-references, so a
//! rename rewrites every definition, reference, and import of a symbol. These
//! tests apply the returned `WorkspaceEdit` to the on-disk sources and then
//! re-analyze the package with `ambient-analysis` to prove the rename
//! round-trips to a still-clean package.

use std::fs;
use std::path::{Path, PathBuf};

use ambient_analysis::package::AnalysisPackage;
use ambient_lsp::test_harness::TestClient;
use lsp_types::{PrepareRenameResponse, TextEdit, Uri, WorkspaceEdit};
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    client: TestClient,
    root: PathBuf,
}

impl Fixture {
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
        let main = root.join(main_file);
        client.open_document(uri_for(&main), fs::read_to_string(&main).unwrap().as_str());

        Fixture {
            _temp: temp,
            client,
            root,
        }
    }

    fn uri(&self, rel: &str) -> Uri {
        uri_for(&self.root.join(rel))
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.root.join(rel)).expect("read source")
    }

    /// Re-open the package from disk and assert every module type-checks
    /// clean — the round-trip check after applying a rename.
    fn assert_package_clean(&self) {
        let package = AnalysisPackage::open(&self.root).expect("open package");
        for (path, result) in package.analyze_all() {
            assert!(
                result.diagnostics().is_empty(),
                "unexpected diagnostics in {path}: {:?}",
                result.diagnostics()
            );
        }
    }
}

fn uri_for(path: &Path) -> Uri {
    format!("file://{}", path.to_string_lossy())
        .parse()
        .expect("parse uri")
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
    byte_edits.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out = content.to_string();
    for (start, end, text) in byte_edits {
        out.replace_range(start..end, text);
    }
    out
}

/// Apply a `WorkspaceEdit` to the on-disk files it names.
fn apply_workspace_edit(edit: &WorkspaceEdit) {
    let changes = edit.changes.as_ref().expect("changes present");
    for (uri, edits) in changes {
        let path = uri
            .as_str()
            .strip_prefix("file://")
            .expect("file uri")
            .to_string();
        let content = fs::read_to_string(&path).expect("read edited file");
        fs::write(&path, apply_edits(&content, edits)).expect("write edited file");
    }
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
    apply_workspace_edit(&edit);

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
    apply_workspace_edit(&edit);

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

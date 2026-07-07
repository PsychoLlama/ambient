//! CLI/LSP diagnostic parity.
//!
//! The language server must report exactly what `ambient check` reports:
//! same messages, same spans, same suppression policy. Both frontends
//! render `ambient_analysis::AnalysisResult::diagnostics()`, so these
//! tests guard the seam — if either side grows its own opinion about
//! what is an error, they fail.

use std::fs;
use std::path::Path;

use ambient_lsp::Document;
use ambient_lsp::test_harness::TestClient;
use lsp_types::Uri;
use tempfile::TempDir;

/// A normalized diagnostic: what the user actually sees.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Normalized {
    start: (u32, u32),
    end: (u32, u32),
    message: String,
}

/// What the shared analysis pipeline (rendered by `ambient check`)
/// reports for one module, normalized to line/character positions.
fn analysis_diagnostics(root: &Path, module_key: &str) -> Vec<Normalized> {
    let package =
        ambient_analysis::package::AnalysisPackage::open(root).expect("package should open");
    let results = package.analyze_all();
    let result = results
        .get(module_key)
        .unwrap_or_else(|| panic!("module `{module_key}` not analyzed"));
    let module = &package.modules[module_key];

    let uri: Uri = format!("file://{}", package.file_for_module(&module.path).display())
        .parse()
        .expect("valid uri");
    let doc = Document::new(uri, 0, module.source.clone());

    result
        .diagnostics()
        .into_iter()
        .map(|d| {
            // The LSP renderer appends notes to the message; mirror it.
            let mut message = d.message.clone();
            if let Some(note) = &d.note {
                message.push_str("\nnote: ");
                message.push_str(note);
            }
            Normalized {
                start: doc.offset_to_position(d.span.start as usize),
                end: doc.offset_to_position(d.span.end as usize),
                message,
            }
        })
        .collect()
}

/// What the LSP server publishes for one open file.
fn lsp_diagnostics(client: &TestClient, uri: &Uri) -> Vec<Normalized> {
    client
        .get_diagnostics(uri)
        .into_iter()
        .map(|d| Normalized {
            start: (d.range.start.line, d.range.start.character),
            end: (d.range.end.line, d.range.end.character),
            message: d.message,
        })
        .collect()
}

/// Write a package to disk and assert that, for every module, the LSP
/// publishes exactly the diagnostics the shared analysis computes.
fn assert_parity(files: &[(&str, &str)]) {
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path();
    fs::write(
        root.join("ambient.toml"),
        "[package]\nname = \"parity\"\nversion = \"0.1.0\"\n\n[build]\nsrc = \"src\"\n",
    )
    .expect("write manifest");
    fs::create_dir_all(root.join("src")).expect("create src");
    for (name, content) in files {
        let path = root.join("src").join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create module dir");
        }
        fs::write(path, content).expect("write module");
    }

    let mut client = TestClient::new();
    let uris: Vec<(String, Uri)> = files
        .iter()
        .map(|(name, content)| {
            let uri: Uri = format!("file://{}", root.join("src").join(name).display())
                .parse()
                .expect("valid uri");
            client.open_document(uri.clone(), content);
            // Module keys are dotted paths; the root module is "main".
            let module_key = name.trim_end_matches(".ab").replace('/', ".");
            (module_key, uri)
        })
        .collect();

    // Let the server finish analyzing all opened documents.
    std::thread::sleep(std::time::Duration::from_millis(200));
    client.process_notifications();

    for (module_key, uri) in &uris {
        let mut expected = analysis_diagnostics(root, module_key);
        let mut actual = lsp_diagnostics(&client, uri);
        expected.sort();
        actual.sort();
        assert_eq!(
            actual, expected,
            "LSP and `ambient check` disagree for module `{module_key}`"
        );
    }

    client.shutdown();
}

#[test]
fn clean_package_reports_nothing_on_both_sides() {
    assert_parity(&[
        (
            "main.ab",
            "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
        ),
        ("utils.ab", "pub fn helper(): Number { 1 }\n"),
    ]);
}

#[test]
fn bare_prelude_names_match() {
    // A module using bare prelude names (`Some`/`None`/`Ok`/`Err`,
    // `Option`/`Result`, `Number`) with no `use` must analyze identically
    // on both frontends — the prelude injection is in the shared layer, so
    // the LSP sees it exactly as `ambient check` does.
    assert_parity(&[(
        "main.ab",
        "fn unwrap(o: Option<Number>): Number { match o { Some(n) => n, None => 0 } }\n\
         pub fn run(): Result<Number, Number> { Ok(unwrap(Some(1))) }\n",
    )]);
}

#[test]
fn type_errors_match() {
    assert_parity(&[(
        "main.ab",
        "pub fn run(): String { 42 }\nfn extra(): Number { \"nope\" }\n",
    )]);
}

#[test]
fn undefined_type_annotation_errors_match() {
    // An undefined type in an annotation is now a first-class diagnostic
    // (`undefined type: Strng`). Both frontends render it from the shared
    // layer, so the LSP must show exactly the same message and span as
    // `ambient check` — including no secondary cascade.
    assert_parity(&[("main.ab", "pub fn run(x: Strng): Number { 1 }\n")]);
}

#[test]
fn parse_errors_match_and_suppress_type_errors() {
    // `bad` carries a type error, `broken` a parse error. Both frontends
    // must report only the parse error (and both must report it — the
    // old LSP went completely dark past the first syntax error).
    assert_parity(&[(
        "main.ab",
        "pub fn run(): String { 42 }\n\nfn broken(\n\nfn also_broken(]\n",
    )]);
}

#[test]
fn cross_module_import_errors_match() {
    assert_parity(&[
        (
            "main.ab",
            "use pkg::utils::nonexistent;\npub fn run(): Number { 1 }\n",
        ),
        (
            "utils.ab",
            "pub fn helper(): Number { 1 }\nfn private_one(): Number { 2 }\n",
        ),
        (
            "other.ab",
            "use pkg::utils::private_one;\npub fn go(): Number { private_one() }\n",
        ),
    ]);
}

#[test]
fn broken_dependency_still_resolves_for_importers() {
    // utils has one broken function; main imports the surviving one.
    // Both sides must agree: utils reports its parse error, main is
    // clean because the partial module still exports `helper`.
    assert_parity(&[
        (
            "main.ab",
            "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
        ),
        ("utils.ab", "fn broken(\n\npub fn helper(): Number { 1 }\n"),
    ]);
}

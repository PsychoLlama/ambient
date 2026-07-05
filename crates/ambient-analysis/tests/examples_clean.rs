//! Every shipped example package must analyze clean.
//!
//! `analyze_all` is the exact computation behind both `ambient check`
//! and the language server's diagnostics, so this is the corpus-level
//! guard: if the analysis pipeline regresses (or an example rots), this
//! fails with the offending module and diagnostics.

use std::path::PathBuf;

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
}

#[test]
fn all_examples_analyze_clean() {
    let examples = examples_dir();
    let mut checked = 0;

    let entries = std::fs::read_dir(&examples).expect("examples directory should exist");
    for entry in entries.flatten() {
        let root = entry.path();
        if !root.join("ambient.toml").exists() {
            continue;
        }

        let package = ambient_analysis::package::AnalysisPackage::open(&root)
            .unwrap_or_else(|e| panic!("failed to open {}: {e}", root.display()));
        for (module, result) in package.analyze_all() {
            let diagnostics = result.diagnostics();
            assert!(
                diagnostics.is_empty(),
                "{}::{module} has diagnostics: {diagnostics:#?}",
                root.file_name().unwrap_or_default().to_string_lossy(),
            );
        }
        checked += 1;
    }

    assert!(
        checked > 5,
        "expected to check several examples, got {checked}"
    );
}

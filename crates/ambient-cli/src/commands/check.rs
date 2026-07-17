//! Check command implementation.
//!
//! Runs the shared analysis pipeline (`ambient-analysis`) — the same
//! computation the language server reports from — so `ambient check` and
//! the editor can never disagree about what is an error.

use std::path::Path;

use anyhow::{Result, bail};

use ambient_analysis::package::AnalysisPackage;

use crate::diagnostic::print_diagnostic;

/// Check an Ambient source file or package for errors.
///
/// - A directory checks the whole package rooted there.
/// - A file inside a package checks the whole package (imports need the
///   package context anyway) and reports every module's diagnostics.
/// - A bare file outside any package checks stand-alone against the core
///   and platform preludes.
pub fn cmd_check(path: &Path) -> Result<()> {
    if path.is_dir() {
        return check_package_at(path);
    }

    if let Some(package) = AnalysisPackage::discover(path) {
        return check_package_at(package.root());
    }

    check_single_file(path)
}

/// Check every module of the package rooted at `root`.
fn check_package_at(root: &Path) -> Result<()> {
    let package = AnalysisPackage::open(root).map_err(|e| anyhow::anyhow!(e))?;
    if package.modules.is_empty() {
        bail!("no .ab files found under {}", package.src_dir().display());
    }

    let results = package.analyze_all();

    // Report in deterministic module-path order.
    let mut module_keys: Vec<_> = results.keys().collect();
    module_keys.sort();

    let mut error_count = 0;
    for key in module_keys {
        let result = &results[key];
        let diagnostics = result.diagnostics();
        if diagnostics.is_empty() {
            continue;
        }

        let module = &package.modules[key];
        let file = package.file_for_module(&module.path);
        for diagnostic in &diagnostics {
            print_diagnostic(&module.source, &file, diagnostic);
        }
        error_count += diagnostics.len();
    }

    if error_count == 0 {
        eprintln!("No errors found in {}", root.display());
        Ok(())
    } else {
        bail!("Found {error_count} error(s) in {}", root.display())
    }
}

/// Check a stand-alone file with no package context.
fn check_single_file(file: &Path) -> Result<()> {
    let source = super::read_source(file)?;
    let result = ambient_analysis::analyze(&source);
    let diagnostics = result.diagnostics();

    if diagnostics.is_empty() {
        eprintln!("No errors found in {}", file.display());
        Ok(())
    } else {
        for diagnostic in &diagnostics {
            print_diagnostic(&source, file, diagnostic);
        }
        bail!("Found {} error(s) in {}", diagnostics.len(), file.display())
    }
}

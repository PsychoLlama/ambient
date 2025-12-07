//! Check command implementation.

use std::path::Path;

use anyhow::{bail, Result};

use super::read_source;
use crate::diagnostic::print_diagnostic;

/// Check an Ambient source file for errors.
pub fn cmd_check(file: &Path) -> Result<()> {
    let source = read_source(file)?;

    // Parse.
    let module = match ambient_parser::parse(&source) {
        Ok(m) => m,
        Err(e) => {
            print_diagnostic(&source, file, &e);
            bail!("parse error in {}", file.display());
        }
    };

    // Type check.
    let result = ambient_engine::infer::check_module(module);

    if result.is_ok() {
        eprintln!("No errors found in {}", file.display());
        Ok(())
    } else {
        // Format and print errors
        for error in &result.errors {
            print_diagnostic(&source, file, error);
        }
        bail!(
            "Found {} type error(s) in {}",
            result.errors.len(),
            file.display()
        );
    }
}

//! CLI command implementations.
//!
//! Each command is implemented in its own submodule.

mod check;
mod compile;
mod dev;
mod init;
mod run;

pub use check::cmd_check;
pub use compile::cmd_compile;
pub use dev::cmd_dev;
pub use init::cmd_init;
pub use run::cmd_run;

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use ambient_engine::compiler::CompiledModule;

use crate::diagnostic::print_diagnostic;

/// Read source code from a file.
pub fn read_source(file: &Path) -> Result<String> {
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext != "ab" && ext != "ambient" {
        bail!("expected .ab source file, got: {}", file.display());
    }
    fs::read_to_string(file).with_context(|| format!("failed to read {}", file.display()))
}

/// Compile source code to a module.
pub fn compile_source(source: &str, file: &Path) -> Result<CompiledModule> {
    // Parse.
    let module = match ambient_parser::parse(source) {
        Ok(m) => m,
        Err(e) => {
            print_diagnostic(source, file, &e);
            bail!("parse error in {}", file.display());
        }
    };

    // Type check.
    let check_result = ambient_engine::infer::check_module(module);
    if !check_result.is_ok() {
        // Print type errors
        for error in &check_result.errors {
            print_diagnostic(source, file, error);
        }
        bail!(
            "Found {} type error(s) in {}",
            check_result.errors.len(),
            file.display()
        );
    }

    // Compile the type-checked module with debug info.
    let compiled = ambient_engine::compiler::compile_module_with_source(
        &check_result.module,
        source,
        &file.display().to_string(),
    )
    .map_err(|e| anyhow::anyhow!("compile error at {}: {e}", file.display()))?;

    Ok(compiled)
}

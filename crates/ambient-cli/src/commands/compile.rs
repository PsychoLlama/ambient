//! Compile command implementation.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use super::{compile_source, read_source};
use crate::serialize::serialize_module;

/// Compile an Ambient source file.
pub fn cmd_compile(file: &Path, output: Option<&Path>) -> Result<()> {
    let source = read_source(file)?;
    let compiled = compile_source(&source, file)?;

    // Determine output path.
    let output_path = output.map_or_else(
        || file.with_extension("ambient"),
        std::path::Path::to_path_buf,
    );

    // Serialize and write the compiled module.
    // For now, we'll use a simple JSON serialization (can switch to binary later).
    let serialized = serde_json::to_string_pretty(&serialize_module(&compiled))
        .context("failed to serialize")?;
    fs::write(&output_path, serialized).context("failed to write output")?;

    eprintln!("Compiled {} -> {}", file.display(), output_path.display());

    Ok(())
}

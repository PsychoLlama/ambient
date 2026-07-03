//! Compile command implementation.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use ambient_engine::build::{BuildError, build_package};

use super::{compile_source, read_source};

/// Parse source code into an AST (wrapper for ambient_parser::parse).
fn parse_source(source: &str) -> Result<ambient_engine::ast::Module, String> {
    ambient_parser::parse(source).map_err(|e| e.to_string())
}

/// Compile an Ambient source file or package.
///
/// If `file` is a directory with `ambient.toml`, compiles the package.
/// Otherwise, compiles a single source file.
pub fn cmd_compile(file: &Path, output: Option<&Path>) -> Result<()> {
    // Check if this is a package directory
    if file.is_dir() || file.join("ambient.toml").exists() {
        let pkg_path = if file.is_dir() {
            file.to_path_buf()
        } else {
            file.parent().unwrap_or(file).to_path_buf()
        };
        compile_package_cmd(&pkg_path)?;
        return Ok(());
    }

    // Single file compilation
    let source = read_source(file)?;
    let compiled = compile_source(&source, file)?;

    // Determine output path.
    let output_path = output.map_or_else(
        || file.with_extension("ambient"),
        std::path::Path::to_path_buf,
    );

    // Write the compiled module as a binary artifact pack: canonical
    // objects + name bindings + entry point. Self-verifying on load.
    fs::write(&output_path, compiled.to_pack().encode()).context("failed to write output")?;

    eprintln!("Compiled {} -> {}", file.display(), output_path.display());

    Ok(())
}

/// Compile a package and print progress.
fn compile_package_cmd(path: &Path) -> Result<()> {
    let progress_cb = |module: &str, current: usize, total: usize| {
        eprintln!("[{}/{}] Compiling {}", current, total, module);
    };

    let result = build_package(path, parse_source, Some(&progress_cb)).map_err(|e| match e {
        BuildError::PackageOpen(msg) => anyhow::anyhow!("failed to open package: {msg}"),
        BuildError::Parse { module, error } => anyhow::anyhow!("parse error in {module}: {error}"),
        BuildError::TypeCheck { module, errors } => {
            anyhow::anyhow!("type errors in {module}: {}", errors.join(", "))
        }
        BuildError::Compile { module, error } => {
            anyhow::anyhow!("compile error in {module}: {error}")
        }
    })?;

    eprintln!(
        "Compiled {} ({} modules)",
        result.package_name, result.module_count
    );

    Ok(())
}

//! Compile command implementation.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use ambient_engine::build::build_package;

use super::{compile_source, parse_source, read_source};
use crate::diagnostic::report_build_error;

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
        compile_package_cmd(&pkg_path, output)?;
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

/// Compile a package and print progress. With `-o`, write the merged
/// build as a binary artifact pack — every canonical object plus the
/// qualified name bindings, canonical signatures, migration
/// obligations, and entry point. That artifact is both runnable
/// (`ambient run app.ambient`) and deployable: it is exactly the
/// generation pack a remote runtime applies via `Deploy::apply!`.
fn compile_package_cmd(path: &Path, output: Option<&Path>) -> Result<()> {
    let progress_cb = |module: &str, current: usize, total: usize| {
        eprintln!("[{}/{}] Compiling {}", current, total, module);
    };

    let stubs = ambient_platform::stub_natives();
    let result = build_package(
        path,
        parse_source,
        &ambient_engine::build::BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            progress: Some(&progress_cb),
            ..Default::default()
        },
    )
    .map_err(report_build_error)?;

    eprintln!(
        "Compiled {} ({} modules)",
        result.package_name, result.module_count
    );

    if let Some(output_path) = output {
        fs::write(output_path, result.compiled.to_pack().encode())
            .context("failed to write output")?;
        eprintln!("Wrote {} -> {}", result.package_name, output_path.display());
    }

    Ok(())
}

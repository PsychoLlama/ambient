//! Build command implementation.

use std::cell::Cell;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use ambient_engine::build::build_and_persist;

use super::{compile_source, parse_source, read_source};
use crate::diagnostic::report_build_error;

/// Build an Ambient source file or package.
///
/// If `file` is a directory with `ambient.toml`, compiles the package (a
/// workspace root builds every member; `--package` narrows to one).
/// Otherwise, compiles a single source file.
pub fn cmd_build(file: &Path, output: Option<&Path>, package: Option<&str>) -> Result<()> {
    // Check if this is a package directory
    if file.is_dir() || file.join("ambient.toml").exists() {
        let pkg_path = if file.is_dir() {
            file.to_path_buf()
        } else {
            file.parent().unwrap_or(file).to_path_buf()
        };
        build_package_cmd(&pkg_path, output, package)?;
        return Ok(());
    }

    // Otherwise treat the target as a bare source file. `build` is primarily
    // a package command, so when the target is neither a package nor a usable
    // `.ab` file, explain it in those terms rather than leaking the
    // single-file reader's "expected .ab source file" message.
    if !file.exists() {
        bail!(
            "no such build target: {} — `ambient build` takes a package \
             directory (containing ambient.toml) or a bare .ab source file",
            file.display()
        );
    }
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext != "ab" && ext != "ambient" {
        bail!(
            "`{}` is not a build target — `ambient build` takes a package \
             directory (containing ambient.toml) or a bare .ab source file",
            file.display()
        );
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
///
/// The build is cache-aware, mirroring `ambient run`: it reads the
/// package-local store's prior snapshot (a previously-built package
/// compiles warm) and persists its objects + snapshot afterward, so
/// `build` also *feeds* the incremental cache. The written artifact
/// pack is byte-identical warm vs. cold — the cache only replaces
/// recompilation with a store load, never the merged output. (Single-`.ab`-file
/// compilation in [`cmd_build`] has no package store and is left cold.)
fn build_package_cmd(path: &Path, output: Option<&Path>, package: Option<&str>) -> Result<()> {
    // A warm build loads unchanged modules from the store rather than
    // compiling them; the callback reports each honestly ("Cached" vs.
    // "Compiling") and tallies the hits for the summary line, so a fully warm
    // build no longer claims to compile everything.
    let cached_count = Cell::new(0usize);
    let progress_cb = |module: &str, current: usize, total: usize, from_cache: bool| {
        if from_cache {
            cached_count.set(cached_count.get() + 1);
            eprintln!("[{current}/{total}] Cached {module}");
        } else {
            eprintln!("[{current}/{total}] Compiling {module}");
        }
    };

    let stubs = ambient_platform::stub_natives();
    let built = build_and_persist(
        path,
        parse_source,
        ambient_engine::build::BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            progress: Some(&progress_cb),
            package,
            ..Default::default()
        },
    )
    .map_err(report_build_error)?;

    // Persisting the warm-feeding store is best-effort (it is a rebuildable
    // cache), so a failure warns rather than fails the compile.
    if let Err(e) = built.persisted {
        eprintln!("warning: failed to persist build to store: {e}");
    }
    let result = built.result;

    let cached = cached_count.get();
    if cached > 0 {
        eprintln!(
            "Compiled {} ({} modules, {} cached)",
            result.package_name, result.module_count, cached
        );
    } else {
        eprintln!(
            "Compiled {} ({} modules)",
            result.package_name, result.module_count
        );
    }

    if let Some(output_path) = output {
        fs::write(output_path, result.compiled.to_pack().encode())
            .context("failed to write output")?;
        eprintln!("Wrote {} -> {}", result.package_name, output_path.display());
    }

    Ok(())
}

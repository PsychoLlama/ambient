//! Run command implementation.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use ambient_engine::abilities::register_all_standard_abilities;
use ambient_engine::compiler::CompiledModule;
use ambient_engine::format::format_value_colored;
use ambient_engine::module_path::ModulePath;
use ambient_engine::package::{LoadedModule, Package};
use ambient_engine::vm::Vm;

use crate::diagnostic::print_diagnostic;
use crate::serialize::deserialize_module;

/// Run an Ambient package or pre-compiled bytecode.
///
/// If `path` is a directory (or contains an `ambient.toml`), runs the package.
/// If `path` is a `.ambient` file, runs the pre-compiled bytecode.
pub fn cmd_run(path: &Path, entry: &str) -> Result<()> {
    let compiled = load_compiled(path)?;
    run_compiled(&compiled, entry)
}

/// Load a compiled module from a path.
///
/// Handles both packages (directories with `ambient.toml`) and
/// pre-compiled `.ambient` files.
fn load_compiled(path: &Path) -> Result<CompiledModule> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    if ext == "ambient" {
        // Load pre-compiled bytecode.
        let contents = fs::read_to_string(path).context("failed to read file")?;
        let serialized: crate::serialize::SerializedModule =
            serde_json::from_str(&contents).context("failed to parse bytecode file")?;
        deserialize_module(&serialized)
    } else if path.is_dir() || path.join("ambient.toml").exists() {
        // Load package.
        compile_package(path)
    } else {
        bail!(
            "expected a directory with ambient.toml or a .ambient file, got: {}",
            path.display()
        );
    }
}

/// Compile a package from its root directory.
fn compile_package(path: &Path) -> Result<CompiledModule> {
    // Open package (validates manifest and entry point).
    let mut pkg = Package::open(path)
        .with_context(|| format!("failed to open package at {}", path.display()))?;

    // Load and compile the main module.
    let main_path = ModulePath::root();
    let loaded = load_module(&pkg, &main_path)?;
    pkg.add_module(loaded);

    // Get the loaded module and compile it.
    let loaded = pkg
        .get_module(&main_path)
        .ok_or_else(|| anyhow::anyhow!("main module not loaded"))?;

    compile_loaded_module(loaded, &pkg.module_file_path(&main_path))
}

/// Load a single module from a package.
fn load_module(pkg: &Package, path: &ModulePath) -> Result<LoadedModule> {
    let source = pkg.read_module_source(path)?;
    let file_path = pkg.module_file_path(path);

    // Parse the module.
    let ast = match ambient_parser::parse(&source) {
        Ok(m) => m,
        Err(e) => {
            print_diagnostic(&source, &file_path, &e);
            bail!("parse error in {}", file_path.display());
        }
    };

    Ok(LoadedModule {
        path: path.clone(),
        source,
        ast,
    })
}

/// Compile a loaded module to bytecode.
fn compile_loaded_module(loaded: &LoadedModule, file_path: &Path) -> Result<CompiledModule> {
    // Type check.
    let check_result = ambient_engine::infer::check_module(loaded.ast.clone());
    if !check_result.is_ok() {
        for error in &check_result.errors {
            print_diagnostic(&loaded.source, file_path, error);
        }
        bail!(
            "Found {} type error(s) in {}",
            check_result.errors.len(),
            file_path.display()
        );
    }

    // Compile with debug info.
    let compiled = ambient_engine::compiler::compile_module_with_source(
        &check_result.module,
        &loaded.source,
        &file_path.display().to_string(),
    )
    .map_err(|e| anyhow::anyhow!("compile error at {}: {e}", file_path.display()))?;

    Ok(compiled)
}

/// Run a compiled module.
fn run_compiled(compiled: &CompiledModule, entry: &str) -> Result<()> {
    // Create and configure VM.
    let mut vm = Vm::new();

    // Register standard ability handlers.
    register_all_standard_abilities(&mut vm);

    // Load all functions into the VM.
    for func in compiled.functions.values() {
        vm.load_function(func.clone());
    }

    // Find entry point.
    let entry_hash = compiled
        .function_names
        .get(entry)
        .ok_or_else(|| anyhow::anyhow!("entry function `{entry}` not found"))?;

    // Execute with stack trace support.
    let result = vm.call_with_trace(entry_hash, Vec::new());

    match result {
        Ok(value) => {
            // Print result if not unit.
            if !matches!(value, ambient_engine::value::Value::Unit) {
                println!("{}", format_value_colored(&value));
            }
            Ok(())
        }
        Err(runtime_error) => {
            // Print rich error with stack trace.
            eprintln!("{runtime_error}");
            bail!("runtime error");
        }
    }
}

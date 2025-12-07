//! Run command implementation.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use ambient_engine::abilities::register_all_standard_abilities;
use ambient_engine::compiler::CompiledModule;
use ambient_engine::format::format_value_colored;
use ambient_engine::manifest::Manifest;
use ambient_engine::vm::Vm;

use super::{compile_source, read_source};
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
    // Find manifest
    let (manifest, root) =
        Manifest::find(path).with_context(|| format!("no package found at {}", path.display()))?;

    // Find main.ab
    let main_path = root.join(&manifest.src_dir).join("main.ab");
    if !main_path.exists() {
        bail!(
            "entry point not found: {}\nPackages must have a main.ab file in the src directory.",
            main_path.display()
        );
    }

    // For now, just compile main.ab (Phase 2 will add multi-file support)
    let source = read_source(&main_path)?;
    compile_source(&source, &main_path)
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

//! Run command implementation.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use ambient_engine::abilities::register_all_standard_abilities;
use ambient_engine::vm::Vm;

use super::{compile_source, read_source};
use crate::serialize::deserialize_module;

/// Run an Ambient program.
pub fn cmd_run(file: &Path, entry: &str) -> Result<()> {
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");

    let compiled = if ext == "ambient" {
        // Load pre-compiled bytecode.
        let contents = fs::read_to_string(file).context("failed to read file")?;
        let serialized: crate::serialize::SerializedModule =
            serde_json::from_str(&contents).context("failed to parse bytecode file")?;
        deserialize_module(&serialized)?
    } else {
        // Compile from source.
        let source = read_source(file)?;
        compile_source(&source, file)?
    };

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
                println!("{value:?}");
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

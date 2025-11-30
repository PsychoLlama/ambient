//! Ambient programming language CLI.
//!
//! This is the main entry point for the `ambient` command-line tool.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use clap::Parser;

use ambient_engine::abilities::{register_console, ConsoleConfig};
use ambient_engine::compiler::{compile_module, CompiledModule};
use ambient_engine::vm::Vm;

mod cli;

use cli::{Args, Command};

pub fn main() -> Result<()> {
    let args = Args::parse();

    match args.command {
        Command::Compile { file, output } => cmd_compile(&file, output.as_deref())?,
        Command::Run { file, entry } => cmd_run(&file, &entry)?,
        Command::Check { file } => cmd_check(&file)?,
        Command::Ast { file } => cmd_ast(&file)?,
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Commands
// ─────────────────────────────────────────────────────────────────────────────

/// Compile an Ambient source file.
fn cmd_compile(file: &Path, output: Option<&Path>) -> Result<()> {
    let source = read_source(file)?;
    let compiled = compile_source(&source, file)?;

    // Determine output path.
    let output_path = output.map_or_else(
        || file.with_extension("ambient"),
        std::path::Path::to_path_buf,
    );

    // Serialize and write the compiled module.
    // For now, we'll use a simple JSON serialization (can switch to binary later).
    let serialized =
        serde_json::to_string_pretty(&serialize_module(&compiled)).context("failed to serialize")?;
    fs::write(&output_path, serialized).context("failed to write output")?;

    eprintln!(
        "Compiled {} -> {}",
        file.display(),
        output_path.display()
    );

    Ok(())
}

/// Run an Ambient program.
fn cmd_run(file: &Path, entry: &str) -> Result<()> {
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");

    let compiled = if ext == "ambient" {
        // Load pre-compiled bytecode.
        let contents = fs::read_to_string(file).context("failed to read file")?;
        let serialized: SerializedModule =
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
    register_console(&mut vm, ConsoleConfig::default());

    // Load all functions into the VM.
    for func in compiled.functions.values() {
        vm.load_function(func.clone());
    }

    // Find entry point.
    let entry_hash = compiled.function_names.get(entry).ok_or_else(|| {
        anyhow::anyhow!("entry function `{entry}` not found")
    })?;

    // Execute.
    let result = vm
        .call(entry_hash, Vec::new())
        .map_err(|e| anyhow::anyhow!("runtime error: {e}"))?;

    // Print result if not unit.
    if !matches!(result, ambient_engine::value::Value::Unit) {
        println!("{result:?}");
    }

    Ok(())
}

/// Check an Ambient source file for errors.
fn cmd_check(file: &Path) -> Result<()> {
    let source = read_source(file)?;

    // Parse.
    let _module =
        ambient_parser::parse(&source).map_err(|e| anyhow::anyhow!("parse error: {e}"))?;

    // TODO: Type checking is not yet implemented at the module level.
    // For now, we just check that the file parses successfully.

    eprintln!("No errors found in {}", file.display());

    Ok(())
}

/// Parse and dump the AST.
fn cmd_ast(file: &Path) -> Result<()> {
    let source = read_source(file)?;

    let module =
        ambient_parser::parse(&source).map_err(|e| anyhow::anyhow!("parse error: {e}"))?;

    println!("{module:#?}");

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Pipeline Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Read source code from a file.
fn read_source(file: &Path) -> Result<String> {
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext != "ab" && ext != "ambient" {
        bail!("expected .ab source file, got: {}", file.display());
    }
    fs::read_to_string(file).with_context(|| format!("failed to read {}", file.display()))
}

/// Compile source code to a module.
fn compile_source(source: &str, file: &Path) -> Result<CompiledModule> {
    // Parse.
    let module =
        ambient_parser::parse(source).map_err(|e| anyhow::anyhow!("parse error at {}: {e}", file.display()))?;

    // TODO: Type checking is not yet implemented at the module level.
    // The compiler will catch some type-related issues, but full type inference
    // for modules will be added in a future milestone.

    // Compile.
    let compiled =
        compile_module(&module).map_err(|e| anyhow::anyhow!("compile error at {}: {e}", file.display()))?;

    Ok(compiled)
}

// ─────────────────────────────────────────────────────────────────────────────
// Serialization (temporary - for .ambient files)
// ─────────────────────────────────────────────────────────────────────────────

use serde::{Deserialize, Serialize};

/// Serialized module format.
#[derive(Debug, Serialize, Deserialize)]
struct SerializedModule {
    functions: Vec<SerializedFunction>,
    function_names: Vec<(String, String)>, // (name, hash_hex)
    entry_point: Option<String>,           // hash_hex
}

/// Serialized function format.
#[derive(Debug, Serialize, Deserialize)]
struct SerializedFunction {
    hash: String, // hex
    bytecode: Vec<u8>,
    constants: Vec<SerializedValue>,
    local_count: u16,
    param_count: u8,
    dependencies: Vec<String>, // hex hashes
}

/// Serialized value format.
#[derive(Debug, Serialize, Deserialize)]
enum SerializedValue {
    Unit,
    Bool(bool),
    Number(f64),
    String(String),
    FunctionRef(String), // hex hash
    // Tuples and records not typically in constant pools
}

fn serialize_module(module: &CompiledModule) -> SerializedModule {
    SerializedModule {
        functions: module
            .functions
            .values()
            .map(|f| SerializedFunction {
                hash: f.hash.to_hex().to_string(),
                bytecode: f.bytecode.clone(),
                constants: f.constants.iter().map(serialize_value).collect(),
                local_count: f.local_count,
                param_count: f.param_count,
                dependencies: f.dependencies.iter().map(|h| h.to_hex().to_string()).collect(),
            })
            .collect(),
        function_names: module
            .function_names
            .iter()
            .map(|(name, hash)| (name.to_string(), hash.to_hex().to_string()))
            .collect(),
        entry_point: module.entry_point.map(|h| h.to_hex().to_string()),
    }
}

fn serialize_value(value: &ambient_engine::value::Value) -> SerializedValue {
    use ambient_engine::value::Value;
    match value {
        Value::Unit => SerializedValue::Unit,
        Value::Bool(b) => SerializedValue::Bool(*b),
        Value::Number(n) => SerializedValue::Number(*n),
        Value::String(s) => SerializedValue::String((**s).clone()),
        Value::FunctionRef(h) => SerializedValue::FunctionRef(h.to_hex().to_string()),
        // These shouldn't appear in constant pools
        Value::Tuple(_) | Value::Record(_) | Value::SuspendedAbility(_) | Value::Continuation(_) => {
            SerializedValue::Unit
        }
    }
}

fn deserialize_module(serialized: &SerializedModule) -> Result<CompiledModule> {
    use ambient_engine::bytecode::CompiledFunction;
    use std::collections::HashMap;
    use std::sync::Arc;

    let mut functions = HashMap::new();
    let mut function_names = HashMap::new();

    for sf in &serialized.functions {
        let hash = blake3::Hash::from_hex(&sf.hash).context("invalid hash")?;
        let constants: Vec<ambient_engine::value::Value> = sf
            .constants
            .iter()
            .map(deserialize_value)
            .collect::<Result<_>>()?;
        let dependencies: Vec<blake3::Hash> = sf
            .dependencies
            .iter()
            .map(|h| blake3::Hash::from_hex(h).context("invalid dependency hash"))
            .collect::<Result<_>>()?;

        // Create function with the stored hash (we can't recompute since it depends on content)
        let func = CompiledFunction {
            hash,
            bytecode: sf.bytecode.clone(),
            constants,
            local_count: sf.local_count,
            param_count: sf.param_count,
            dependencies,
        };
        functions.insert(hash, func);
    }

    for (name, hash_str) in &serialized.function_names {
        let hash = blake3::Hash::from_hex(hash_str).context("invalid hash")?;
        function_names.insert(Arc::from(name.as_str()), hash);
    }

    let entry_point = serialized
        .entry_point
        .as_ref()
        .map(|h| blake3::Hash::from_hex(h).context("invalid entry point hash"))
        .transpose()?;

    Ok(CompiledModule {
        functions,
        function_names,
        entry_point,
    })
}

fn deserialize_value(sv: &SerializedValue) -> Result<ambient_engine::value::Value> {
    use ambient_engine::value::Value;
    use std::sync::Arc;

    Ok(match sv {
        SerializedValue::Unit => Value::Unit,
        SerializedValue::Bool(b) => Value::Bool(*b),
        SerializedValue::Number(n) => Value::Number(*n),
        SerializedValue::String(s) => Value::String(Arc::new(s.clone())),
        SerializedValue::FunctionRef(h) => {
            Value::FunctionRef(blake3::Hash::from_hex(h).context("invalid function ref hash")?)
        }
    })
}

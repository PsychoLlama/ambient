//! Ambient programming language CLI.
//!
//! This is the main entry point for the `ambient` command-line tool.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use clap::Parser;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use ambient_engine::abilities::{
    format_value, register_all_standard_abilities, register_console, ConsoleConfig,
};
use ambient_engine::compiler::{compile_expression, compile_module, CompiledModule};
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
        Command::Repl => cmd_repl()?,
        Command::Lsp => cmd_lsp()?,
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
    let serialized = serde_json::to_string_pretty(&serialize_module(&compiled))
        .context("failed to serialize")?;
    fs::write(&output_path, serialized).context("failed to write output")?;

    eprintln!("Compiled {} -> {}", file.display(), output_path.display());

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
    let entry_hash = compiled
        .function_names
        .get(entry)
        .ok_or_else(|| anyhow::anyhow!("entry function `{entry}` not found"))?;

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
    let module = match ambient_parser::parse(&source) {
        Ok(m) => m,
        Err(e) => {
            print_parse_error(&source, file, &e);
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
            print_type_error(&source, file, error);
        }
        bail!(
            "Found {} type error(s) in {}",
            result.errors.len(),
            file.display()
        );
    }
}

/// Parse and dump the AST.
fn cmd_ast(file: &Path) -> Result<()> {
    let source = read_source(file)?;

    let module = match ambient_parser::parse(&source) {
        Ok(m) => m,
        Err(e) => {
            print_parse_error(&source, file, &e);
            bail!("parse error in {}", file.display());
        }
    };

    println!("{module:#?}");

    Ok(())
}

/// Run the LSP server.
fn cmd_lsp() -> Result<()> {
    use std::io::{stdin, stdout};
    ambient_lsp::run_server(stdin().lock(), stdout().lock()).context("LSP server error")
}

/// Run the interactive REPL.
fn cmd_repl() -> Result<()> {
    eprintln!("Ambient REPL v0.1.0");
    eprintln!("Type expressions to evaluate. Type :help for commands, :quit to exit.\n");

    let mut rl = DefaultEditor::new().context("failed to initialize readline")?;
    let mut vm = Vm::new();

    // Register standard abilities for the REPL.
    register_all_standard_abilities(&mut vm);

    // Track REPL evaluation count for unique function names.
    let mut eval_count: u64 = 0;

    loop {
        // Flush stdout before reading (in case any output is buffered).
        let _ = io::stdout().flush();

        let readline = rl.readline("ambient> ");
        match readline {
            Ok(line) => {
                let line = line.trim();

                // Skip empty lines.
                if line.is_empty() {
                    continue;
                }

                // Add to history.
                let _ = rl.add_history_entry(line);

                // Handle REPL commands.
                if line.starts_with(':') {
                    match handle_repl_command(line) {
                        ReplCommand::Help => print_repl_help(),
                        ReplCommand::Quit => {
                            eprintln!("Goodbye!");
                            break;
                        }
                        ReplCommand::Clear => {
                            // Clear the VM state by creating a fresh VM.
                            vm = Vm::new();
                            register_all_standard_abilities(&mut vm);
                            eval_count = 0;
                            eprintln!("State cleared.");
                        }
                        ReplCommand::Unknown(cmd) => {
                            eprintln!("Unknown command: {cmd}");
                            eprintln!("Type :help for available commands.");
                        }
                    }
                    continue;
                }

                // Evaluate the expression.
                eval_count += 1;
                match eval_repl_line(&mut vm, line, eval_count) {
                    Ok(Some(value)) => {
                        println!("{}", format_value(&value));
                    }
                    Ok(None) => {
                        // Unit result, don't print.
                    }
                    Err(e) => {
                        eprintln!("\x1b[1;31merror\x1b[0m: {e}");
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                eprintln!("^C");
                // Continue the REPL on Ctrl+C.
            }
            Err(ReadlineError::Eof) => {
                eprintln!("Goodbye!");
                break;
            }
            Err(err) => {
                bail!("readline error: {err}");
            }
        }
    }

    Ok(())
}

/// REPL command types.
enum ReplCommand {
    Help,
    Quit,
    Clear,
    Unknown(String),
}

/// Parse a REPL command.
fn handle_repl_command(line: &str) -> ReplCommand {
    let cmd = line.trim_start_matches(':').split_whitespace().next();
    match cmd {
        Some("help") | Some("h") | Some("?") => ReplCommand::Help,
        Some("quit") | Some("q") | Some("exit") => ReplCommand::Quit,
        Some("clear") | Some("reset") => ReplCommand::Clear,
        Some(other) => ReplCommand::Unknown(other.to_string()),
        None => ReplCommand::Unknown(String::new()),
    }
}

/// Print REPL help.
fn print_repl_help() {
    eprintln!("REPL Commands:");
    eprintln!("  :help, :h, :?    Show this help message");
    eprintln!("  :quit, :q, :exit Exit the REPL");
    eprintln!("  :clear, :reset   Clear all defined functions and variables");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  1 + 2 * 3        Evaluate an expression");
    eprintln!("  \"hello\"          String literal");
    eprintln!("  (1, 2, 3)        Tuple");
    eprintln!("  {{ x: 1, y: 2 }}   Record");
}

/// Evaluate a REPL line.
fn eval_repl_line(
    vm: &mut Vm,
    line: &str,
    eval_count: u64,
) -> Result<Option<ambient_engine::value::Value>, String> {
    // Try to parse as an expression.
    let expr = match ambient_parser::parse_expr(line) {
        Ok(e) => e,
        Err(e) => {
            return Err(format_repl_parse_error(line, &e));
        }
    };

    // Compile the expression.
    let func = compile_expression(&expr).map_err(|e| format!("compile error: {e}"))?;

    // Create a unique name for this evaluation.
    let _func_name = format!("__repl_eval_{eval_count}");
    let func_hash = func.hash;

    // Load the function into the VM.
    vm.load_function(func);

    // Execute the function.
    let result = vm
        .call(&func_hash, Vec::new())
        .map_err(|e| format!("runtime error: {e}"))?;

    // Return the result (None for Unit).
    if matches!(result, ambient_engine::value::Value::Unit) {
        Ok(None)
    } else {
        Ok(Some(result))
    }
}

/// Format a parse error for REPL display (without file path).
fn format_repl_parse_error(line: &str, error: &ambient_parser::ParseError) -> String {
    let start = error.span.start as usize;
    let end = error.span.end as usize;

    // For single-line REPL input, we can show a caret pointing to the error.
    let col = start.min(line.len());
    let underline_len = (end - start).min(line.len().saturating_sub(col)).max(1);

    let spaces = " ".repeat(col);
    let carets = "^".repeat(underline_len);

    let mut msg = format!("{}\n", error.kind);
    msg.push_str(&format!("  {line}\n"));
    msg.push_str(&format!("  {spaces}{carets}"));

    if let Some(ctx) = &error.context {
        msg.push_str(&format!("\n  note: {ctx}"));
    }

    msg
}

// ─────────────────────────────────────────────────────────────────────────────
// Error Formatting
// ─────────────────────────────────────────────────────────────────────────────

/// Print a parse error with source context.
fn print_parse_error(source: &str, file: &Path, error: &ambient_parser::ParseError) {
    let start = error.span.start;
    let end = error.span.end;

    // Find line and column from byte offset
    let (line_num, col, line_start, line_end) = find_line_info(source, start as usize);

    // Extract the line content
    let line_content = &source[line_start..line_end];

    // Print error header
    eprintln!("\x1b[1;31merror\x1b[0m: {}", error.kind);

    // Print location
    eprintln!(
        "  \x1b[1;34m-->\x1b[0m {}:{}:{}",
        file.display(),
        line_num,
        col
    );

    // Print the source line with line number
    let line_num_str = format!("{line_num}");
    let padding = " ".repeat(line_num_str.len());
    eprintln!("   {padding} \x1b[1;34m|\x1b[0m");
    eprintln!(" {line_num_str} \x1b[1;34m|\x1b[0m {line_content}");

    // Print the error underline
    let underline_start = col.saturating_sub(1);
    let underline_len = ((end - start) as usize)
        .min(line_content.len().saturating_sub(underline_start))
        .max(1);
    let spaces = " ".repeat(underline_start);
    let carets = "^".repeat(underline_len);
    eprintln!("   {padding} \x1b[1;34m|\x1b[0m {spaces}\x1b[1;31m{carets}\x1b[0m");

    // Print context if available
    if let Some(ctx) = &error.context {
        eprintln!("   {padding} \x1b[1;34m|\x1b[0m");
        eprintln!("   {padding} \x1b[1;34m= note:\x1b[0m {ctx}");
    }

    eprintln!();
}

/// Print a type error with source context.
fn print_type_error(source: &str, file: &Path, error: &ambient_engine::infer::TypeError) {
    let (start, end) = error.span;

    // Find line and column from byte offset
    let (line_num, col, line_start, line_end) = find_line_info(source, start as usize);

    // Extract the line content
    let line_content = &source[line_start..line_end];

    // Print error header
    eprintln!("\x1b[1;31merror\x1b[0m: {}", error.kind);

    // Print location
    eprintln!(
        "  \x1b[1;34m-->\x1b[0m {}:{}:{}",
        file.display(),
        line_num,
        col
    );

    // Print the source line with line number
    let line_num_str = format!("{line_num}");
    let padding = " ".repeat(line_num_str.len());
    eprintln!("   {padding} \x1b[1;34m|\x1b[0m");
    eprintln!(" {line_num_str} \x1b[1;34m|\x1b[0m {line_content}");

    // Print the error underline
    let underline_start = col.saturating_sub(1);
    let underline_len = ((end - start) as usize)
        .min(line_content.len() - underline_start)
        .max(1);
    let spaces = " ".repeat(underline_start);
    let carets = "^".repeat(underline_len);
    eprintln!("   {padding} \x1b[1;34m|\x1b[0m {spaces}\x1b[1;31m{carets}\x1b[0m");

    // Print context if available
    if let Some(ctx) = &error.context {
        eprintln!("   {padding} \x1b[1;34m|\x1b[0m");
        eprintln!("   {padding} \x1b[1;34m= note:\x1b[0m {ctx}");
    }

    eprintln!();
}

/// Find line number, column, and line bounds for a byte offset.
fn find_line_info(source: &str, offset: usize) -> (usize, usize, usize, usize) {
    let mut line_num = 1;
    let mut line_start = 0;

    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line_num += 1;
            line_start = i + 1;
        }
    }

    // Find end of line
    let line_end = source[line_start..]
        .find('\n')
        .map_or(source.len(), |i| line_start + i);

    let col = offset - line_start + 1;
    (line_num, col, line_start, line_end)
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
    let module = match ambient_parser::parse(source) {
        Ok(m) => m,
        Err(e) => {
            print_parse_error(source, file, &e);
            bail!("parse error in {}", file.display());
        }
    };

    // Type check.
    let check_result = ambient_engine::infer::check_module(module);
    if !check_result.is_ok() {
        // Print type errors
        for error in &check_result.errors {
            print_type_error(source, file, error);
        }
        bail!(
            "Found {} type error(s) in {}",
            check_result.errors.len(),
            file.display()
        );
    }

    // Compile the type-checked module.
    let compiled = compile_module(&check_result.module)
        .map_err(|e| anyhow::anyhow!("compile error at {}: {e}", file.display()))?;

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
                dependencies: f
                    .dependencies
                    .iter()
                    .map(|h| h.to_hex().to_string())
                    .collect(),
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
        Value::Tuple(_)
        | Value::Record(_)
        | Value::SuspendedAbility(_)
        | Value::Continuation(_)
        | Value::Closure(_)
        | Value::Handler(_)
        | Value::List(_)
        | Value::Map(_) => SerializedValue::Unit,
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

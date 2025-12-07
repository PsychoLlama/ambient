//! Ambient programming language CLI.
//!
//! This is the main entry point for the `ambient` command-line tool.

use std::borrow::Cow;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use rustyline::completion::Completer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{Config as RustylineConfig, Editor, Helper};

use ambient_engine::abilities::{format_value, register_all_standard_abilities};
use ambient_engine::compiler::{
    compile_expression_with_context, compile_module_with_source, compile_repl_item, CompiledModule,
    ReplContext, ReplItemKind,
};
use ambient_engine::vm::Vm;

use ambient_parser::ReplInput;

mod cli;
mod diagnostic;

use cli::{Args, Command};
use diagnostic::print_diagnostic;

pub fn main() -> Result<()> {
    let args = Args::parse();

    match args.command {
        Command::Compile { file, output } => cmd_compile(&file, output.as_deref())?,
        Command::Run { file, entry } => cmd_run(&file, &entry)?,
        Command::Check { file } => cmd_check(&file)?,
        Command::Ast { file } => cmd_ast(&file)?,
        Command::Repl => cmd_repl()?,
        Command::Lsp => cmd_lsp()?,
        Command::Dev { file, entry, watch } => cmd_dev(&file, &entry, watch.as_deref())?,
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

/// Check an Ambient source file for errors.
fn cmd_check(file: &Path) -> Result<()> {
    let source = read_source(file)?;

    // Parse.
    let module = match ambient_parser::parse(&source) {
        Ok(m) => m,
        Err(e) => {
            print_diagnostic(&source, file, &e);
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
            print_diagnostic(&source, file, error);
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
            print_diagnostic(&source, file, &e);
            bail!("parse error in {}", file.display());
        }
    };

    println!("{module:#?}");

    Ok(())
}

/// Run the LSP server.
fn cmd_lsp() -> Result<()> {
    ambient_lsp::run_server().context("LSP server error")
}

// ─────────────────────────────────────────────────────────────────────────────
// REPL Syntax Highlighting
// ─────────────────────────────────────────────────────────────────────────────

/// ANSI color codes for syntax highlighting.
mod colors {
    pub const RESET: &str = "\x1b[0m";
    pub const KEYWORD: &str = "\x1b[1;35m"; // Bold magenta
    pub const STRING: &str = "\x1b[32m"; // Green
    pub const NUMBER: &str = "\x1b[33m"; // Yellow
    pub const COMMENT: &str = "\x1b[90m"; // Gray
    pub const OPERATOR: &str = "\x1b[36m"; // Cyan
    pub const BOOLEAN: &str = "\x1b[33m"; // Yellow (same as number)
    pub const ABILITY: &str = "\x1b[1;34m"; // Bold blue
}

/// Keywords in the Ambient language.
const KEYWORDS: &[&str] = &[
    "fn", "pub", "let", "const", "if", "else", "match", "enum", "type", "ability", "use", "with",
    "handle", "resume", "sandbox", "unique",
];

/// Built-in type names and abilities.
const BUILTINS: &[&str] = &[
    "Console",
    "Filesystem",
    "Network",
    "Time",
    "Random",
    "Log",
    "Exception",
    "Async",
    "Option",
    "Result",
    "List",
    "Map",
    "Set",
    "Some",
    "None",
    "Ok",
    "Err",
];

/// Syntax highlighter for the Ambient REPL.
#[derive(Default)]
struct AmbientHighlighter;

impl Highlighter for AmbientHighlighter {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        Cow::Owned(highlight_ambient(line))
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _forced: bool) -> bool {
        // Return true to re-highlight on every character
        true
    }

    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        _default: bool,
    ) -> Cow<'b, str> {
        // Highlight the prompt in bold cyan
        Cow::Owned(format!("\x1b[1;36m{prompt}\x1b[0m"))
    }
}

/// Highlight Ambient source code with ANSI colors.
fn highlight_ambient(input: &str) -> String {
    let mut result = String::with_capacity(input.len() * 2);
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let c = chars[i];

        // Comments
        if c == '/' && i + 1 < len && chars[i + 1] == '/' {
            result.push_str(colors::COMMENT);
            while i < len {
                result.push(chars[i]);
                i += 1;
            }
            result.push_str(colors::RESET);
            continue;
        }

        // Strings
        if c == '"' {
            result.push_str(colors::STRING);
            result.push(c);
            i += 1;
            while i < len {
                let sc = chars[i];
                result.push(sc);
                i += 1;
                if sc == '"' {
                    break;
                }
                if sc == '\\' && i < len {
                    result.push(chars[i]);
                    i += 1;
                }
            }
            result.push_str(colors::RESET);
            continue;
        }

        // Numbers
        if c.is_ascii_digit() {
            result.push_str(colors::NUMBER);
            while i < len && (chars[i].is_ascii_digit() || chars[i] == '.') {
                result.push(chars[i]);
                i += 1;
            }
            result.push_str(colors::RESET);
            continue;
        }

        // Identifiers/keywords
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();

            if KEYWORDS.contains(&word.as_str()) {
                result.push_str(colors::KEYWORD);
                result.push_str(&word);
                result.push_str(colors::RESET);
            } else if word == "true" || word == "false" {
                result.push_str(colors::BOOLEAN);
                result.push_str(&word);
                result.push_str(colors::RESET);
            } else if BUILTINS.contains(&word.as_str()) {
                result.push_str(colors::ABILITY);
                result.push_str(&word);
                result.push_str(colors::RESET);
            } else {
                result.push_str(&word);
            }
            continue;
        }

        // Operators
        if "+-*/%=<>!&|".contains(c) {
            result.push_str(colors::OPERATOR);
            result.push(c);
            // Handle two-character operators
            if i + 1 < len {
                let next = chars[i + 1];
                let is_two_char = matches!(
                    (c, next),
                    ('=' | '!' | '<' | '>', '=')
                        | ('&', '&')
                        | ('|', '|')
                        | ('=', '>')
                        | ('-', '>')
                );
                if is_two_char {
                    result.push(next);
                    i += 1;
                }
            }
            result.push_str(colors::RESET);
            i += 1;
            continue;
        }

        // Other characters pass through
        result.push(c);
        i += 1;
    }

    result
}

/// Helper that combines all the REPL functionality.
#[derive(Default)]
struct AmbientHelper {
    highlighter: AmbientHighlighter,
}

impl Helper for AmbientHelper {}
impl Completer for AmbientHelper {
    type Candidate = String;
}
impl Hinter for AmbientHelper {
    type Hint = String;
}
impl Validator for AmbientHelper {}
impl Highlighter for AmbientHelper {
    fn highlight<'l>(&self, line: &'l str, pos: usize) -> Cow<'l, str> {
        self.highlighter.highlight(line, pos)
    }

    fn highlight_char(&self, line: &str, pos: usize, forced: bool) -> bool {
        self.highlighter.highlight_char(line, pos, forced)
    }

    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        default: bool,
    ) -> Cow<'b, str> {
        self.highlighter.highlight_prompt(prompt, default)
    }
}

/// Get the history file path.
fn get_history_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("ambient").join("repl_history"))
}

/// Run the interactive REPL.
fn cmd_repl() -> Result<()> {
    eprintln!("Ambient REPL v0.1.0");
    eprintln!("Type expressions to evaluate. Type :help for commands, :quit to exit.\n");

    // Configure rustyline with our helper.
    let config = RustylineConfig::builder()
        .auto_add_history(true)
        .max_history_size(1000)
        .expect("valid history size")
        .build();

    let mut rl: Editor<AmbientHelper, DefaultHistory> =
        Editor::with_config(config).context("failed to initialize readline")?;
    rl.set_helper(Some(AmbientHelper::default()));

    // Load history from file.
    if let Some(history_path) = get_history_path() {
        if let Some(parent) = history_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = rl.load_history(&history_path);
    }

    let mut vm = Vm::new();

    // Register standard abilities for the REPL.
    register_all_standard_abilities(&mut vm);

    // Context for tracking defined functions and constants.
    let mut repl_ctx = ReplContext::new();

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
                            repl_ctx = ReplContext::new();
                            eprintln!("State cleared.");
                        }
                        ReplCommand::Unknown(cmd) => {
                            eprintln!("Unknown command: {cmd}");
                            eprintln!("Type :help for available commands.");
                        }
                    }
                    continue;
                }

                // Parse and evaluate the input.
                match eval_repl_input(&mut vm, &mut repl_ctx, line) {
                    Ok(Some(value)) => {
                        println!("{}", format_value(&value));
                    }
                    Ok(None) => {
                        // Unit result or definition, don't print.
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

    // Save history to file.
    if let Some(history_path) = get_history_path() {
        let _ = rl.save_history(&history_path);
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
    eprintln!("Definitions:");
    eprintln!("  fn add(x, y) {{ x + y }}   Define a function");
    eprintln!("  const PI = 3.14159        Define a constant");
    eprintln!();
    eprintln!("Expressions:");
    eprintln!("  1 + 2 * 3        Evaluate an expression");
    eprintln!("  add(1, 2)        Call a defined function");
    eprintln!("  \"hello\"          String literal");
    eprintln!("  (1, 2, 3)        Tuple");
    eprintln!("  {{ x: 1, y: 2 }}   Record");
}

/// Evaluate REPL input (either an expression or a definition).
fn eval_repl_input(
    vm: &mut Vm,
    ctx: &mut ReplContext,
    line: &str,
) -> Result<Option<ambient_engine::value::Value>, String> {
    // Parse the input as either an item or an expression.
    let input = match ambient_parser::parse_repl_input(line) {
        Ok(i) => i,
        Err(e) => {
            return Err(format_repl_parse_error(line, &e));
        }
    };

    match input {
        ReplInput::Item(item) => {
            // Compile the item.
            let compiled =
                compile_repl_item(&item, ctx).map_err(|e| format!("compile error: {e}"))?;

            let name = compiled.name.clone();
            let hash = compiled.function.hash;
            let kind = compiled.kind;

            // Load the function into the VM.
            vm.load_function(compiled.function);

            // Register the name in the context for future references.
            match kind {
                ReplItemKind::Function => ctx.register_function(name.clone(), hash),
                ReplItemKind::Constant => ctx.register_constant(name.clone(), hash),
            }

            // Print confirmation of the definition.
            eprintln!("Defined: {name}");

            // Definitions don't produce a value.
            Ok(None)
        }
        ReplInput::Expr(expr) => {
            // Compile the expression with the current context.
            let func = compile_expression_with_context(&expr, ctx)
                .map_err(|e| format!("compile error: {e}"))?;

            let func_hash = func.hash;

            // Load the function into the VM.
            vm.load_function(func);

            // Execute the function with stack trace support.
            let result = vm
                .call_with_trace(&func_hash, Vec::new())
                .map_err(|e| format!("{e}"))?;

            // Return the result (None for Unit).
            if matches!(result, ambient_engine::value::Value::Unit) {
                Ok(None)
            } else {
                Ok(Some(result))
            }
        }
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
    let compiled =
        compile_module_with_source(&check_result.module, source, &file.display().to_string())
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
        | Value::Map(_)
        | Value::Set(_)
        | Value::Enum(_) => SerializedValue::Unit,
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
            debug_info: None, // Debug info not serialized yet
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

// ─────────────────────────────────────────────────────────────────────────────
// Hot Reload Dev Server
// ─────────────────────────────────────────────────────────────────────────────

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc::channel;
use std::time::Duration;

/// Run an Ambient program with hot reload on file changes.
fn cmd_dev(file: &Path, entry: &str, watch_dirs: Option<&[PathBuf]>) -> Result<()> {
    use std::time::Instant;

    let file = file.canonicalize().context("failed to resolve file path")?;

    // Determine watch directories.
    let watch_paths: Vec<PathBuf> = match watch_dirs {
        Some(dirs) => dirs.to_vec(),
        None => {
            // Default to the file's parent directory.
            vec![file.parent().unwrap_or(Path::new(".")).to_path_buf()]
        }
    };

    eprintln!("\x1b[1;36m[dev]\x1b[0m Watching for changes...");
    for path in &watch_paths {
        eprintln!("      {}", path.display());
    }
    eprintln!();

    // Create a channel to receive file events.
    let (tx, rx) = channel();

    // Create a watcher.
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        Config::default().with_poll_interval(Duration::from_millis(200)),
    )
    .context("failed to create file watcher")?;

    // Start watching directories.
    for path in &watch_paths {
        watcher
            .watch(path, RecursiveMode::Recursive)
            .with_context(|| format!("failed to watch {}", path.display()))?;
    }

    // Initial run.
    run_dev_iteration(&file, entry);

    // Watch for changes.
    let mut last_run = Instant::now();
    let debounce = Duration::from_millis(100);

    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(event) => {
                // Filter to only .ab file changes.
                let is_ab_change = event
                    .paths
                    .iter()
                    .any(|p| p.extension().is_some_and(|ext| ext == "ab"));

                if is_ab_change && last_run.elapsed() > debounce {
                    last_run = Instant::now();
                    eprintln!();
                    eprintln!("\x1b[1;36m[dev]\x1b[0m File changed, reloading...");
                    eprintln!();
                    run_dev_iteration(&file, entry);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Continue waiting.
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                bail!("file watcher disconnected");
            }
        }
    }
}

/// Run a single iteration of the dev server (compile and execute).
fn run_dev_iteration(file: &Path, entry: &str) {
    use std::time::Instant;

    let start = Instant::now();

    // Read source.
    let source = match fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "\x1b[1;31merror\x1b[0m: failed to read {}: {}",
                file.display(),
                e
            );
            return;
        }
    };

    // Compile.
    let compiled = match compile_source(&source, file) {
        Ok(c) => c,
        Err(_) => {
            // Error already printed by compile_source.
            return;
        }
    };

    let compile_time = start.elapsed();

    // Create and configure VM.
    let mut vm = Vm::new();
    register_all_standard_abilities(&mut vm);

    // Load all functions into the VM.
    for func in compiled.functions.values() {
        vm.load_function(func.clone());
    }

    // Find entry point.
    let entry_hash = match compiled.function_names.get(entry) {
        Some(h) => h,
        None => {
            eprintln!("\x1b[1;31merror\x1b[0m: entry function `{entry}` not found");
            return;
        }
    };

    // Execute with stack trace support.
    let run_start = Instant::now();
    match vm.call_with_trace(entry_hash, Vec::new()) {
        Ok(result) => {
            let run_time = run_start.elapsed();
            let formatted = format_value(&result);
            eprintln!();
            eprintln!(
                "\x1b[1;32m[done]\x1b[0m {} (compile: {:?}, run: {:?})",
                formatted, compile_time, run_time
            );
        }
        Err(runtime_error) => {
            eprintln!();
            eprintln!("\x1b[1;31m{runtime_error}\x1b[0m");
        }
    }
}

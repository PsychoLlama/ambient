//! REPL (Read-Eval-Print-Loop) implementation.
//!
//! This module provides an interactive environment for evaluating Ambient code.

mod completer;
mod editor;
mod highlighter;
mod lsp_bridge;

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::{Config as RustylineConfig, Editor, EventHandler, KeyEvent};

use editor::ExternalEditorHandler;

use ambient_engine::abilities::register_all_standard_abilities;
use ambient_engine::compiler::{
    compile_expression_with_context, compile_repl_item, parse_module_exports, ReplContext,
    ReplItemKind,
};
use ambient_engine::format::format_value_colored;
use ambient_engine::manifest::Manifest;
use ambient_engine::value::{ModuleExport, ModuleExportKind, ModuleValue};
use ambient_engine::vm::Vm;
use ambient_parser::ReplInput;

use completer::ReplCompleter;

/// Run the interactive REPL.
pub fn cmd_repl(project_dir: Option<&Path>) -> Result<()> {
    eprintln!("Type :help for commands.");

    // Determine project directory (default to current directory).
    let project_dir = match project_dir {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    // Create shared REPL context for completions.
    let repl_ctx = Arc::new(Mutex::new(ReplContext::new()));

    let mut vm = Vm::new();

    // Register standard abilities for the REPL.
    register_all_standard_abilities(&mut vm);

    // Register built-in modules for introspection and compile core library.
    {
        let mut ctx = repl_ctx.lock().unwrap();
        ctx.register_core_modules();
        ctx.register_ability_modules();

        // Compile and load core library functions into the VM.
        // This allows calling functions like core.list.last() from the REPL.
        compile_and_load_core_library(&mut ctx, &mut vm);
    }

    // Register project modules for introspection.
    register_project_modules(&project_dir, &repl_ctx);

    // Create the completer with project context.
    let completer = ReplCompleter::new(project_dir.clone(), Arc::clone(&repl_ctx));

    // Configure rustyline with our helper.
    let config = RustylineConfig::builder()
        .auto_add_history(true)
        .max_history_size(1000)
        .expect("valid history size")
        .build();

    let mut rl: Editor<ReplCompleter, DefaultHistory> =
        Editor::with_config(config).context("failed to initialize readline")?;
    rl.set_helper(Some(completer));

    // Bind Ctrl+E to open external editor (like bash's edit-and-execute-command).
    rl.bind_sequence(
        KeyEvent::ctrl('E'),
        EventHandler::Conditional(Box::new(ExternalEditorHandler)),
    );

    // Load history from file.
    if let Some(history_path) = get_history_path() {
        if let Some(parent) = history_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = rl.load_history(&history_path);
    }

    loop {
        // Flush stdout before reading (in case any output is buffered).
        let _ = io::stdout().flush();

        let readline = rl.readline("> ");
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
                            break;
                        }
                        ReplCommand::Clear => {
                            // Clear the VM state by creating a fresh VM.
                            vm = Vm::new();
                            register_all_standard_abilities(&mut vm);
                            *repl_ctx.lock().unwrap() = ReplContext::new();
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
                let mut ctx = repl_ctx.lock().unwrap();
                match eval_repl_input(&mut vm, &mut ctx, line) {
                    Ok(Some(value)) => {
                        println!("{}", format_value_colored(&value));
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
    eprintln!("Key Bindings:");
    eprintln!("  Ctrl+E           Edit current line in $EDITOR");
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
    // Check if the input is a module path (e.g., "core", "core.math", "Console").
    // This allows users to inspect modules by just typing their name.
    let trimmed = line.trim();
    if let Some(module) = ctx.get_module(trimmed) {
        return Ok(Some(ambient_engine::value::Value::Module(
            std::sync::Arc::clone(module),
        )));
    }

    // Check if the input is a module member path (e.g., "core.list.first").
    // This allows users to inspect functions and constants from modules.
    if let Some(kind) = ctx.get_module_member(trimmed) {
        use ambient_engine::value::ModuleMemberRef;
        return Ok(Some(ambient_engine::value::Value::ModuleMember(
            std::sync::Arc::new(ModuleMemberRef {
                path: trimmed.into(),
                kind,
            }),
        )));
    }

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

/// Get the history file path.
fn get_history_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("ambient").join("repl_history"))
}

/// Register project modules for introspection in the REPL.
fn register_project_modules(project_dir: &Path, repl_ctx: &Arc<Mutex<ReplContext>>) {
    // Find the manifest by walking up from project_dir
    let mut current = project_dir;
    let manifest_path = loop {
        let candidate = current.join("ambient.toml");
        if candidate.exists() {
            break Some(candidate);
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break None,
        }
    };

    let Some(manifest_path) = manifest_path else {
        return;
    };

    let Ok(manifest) = Manifest::from_file(&manifest_path) else {
        return;
    };

    let project_root = manifest_path.parent().unwrap();
    let src_dir = project_root.join(&manifest.src_dir);

    if !src_dir.is_dir() {
        return;
    }

    // Discover all .ab files and register them
    let modules = discover_project_modules(&src_dir, &src_dir);

    let mut ctx = repl_ctx.lock().unwrap();

    // Register "pkg" as the root module containing all project modules
    let pkg_exports: Vec<ModuleExport> = modules
        .iter()
        .filter_map(|(name, _)| {
            // Only include top-level modules (no dots in name)
            if !name.contains('.') {
                Some(ModuleExport::new(name.as_str(), ModuleExportKind::Module))
            } else {
                None
            }
        })
        .collect();

    if !pkg_exports.is_empty() {
        ctx.register_module("pkg", ModuleValue::new("pkg", pkg_exports));
    }

    // Register each module with its exports
    for (module_name, source) in modules {
        let exports = parse_module_exports(&source);
        let path = format!("pkg.{module_name}");
        ctx.register_module(path.clone(), ModuleValue::new(path, exports));

        // Also register without pkg prefix for convenience (e.g., "math_utils" directly)
        let exports = parse_module_exports(&source);
        ctx.register_module(module_name.clone(), ModuleValue::new(module_name, exports));
    }
}

/// Recursively discover .ab files and return (module_path, source) pairs.
fn discover_project_modules(dir: &Path, src_root: &Path) -> Vec<(String, String)> {
    let mut modules = Vec::new();

    let Ok(entries) = fs::read_dir(dir) else {
        return modules;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            modules.extend(discover_project_modules(&path, src_root));
        } else if path.extension().is_some_and(|ext| ext == "ab") {
            if let Some(module_path) = path_to_module_name(&path, src_root) {
                // Skip "main" as it's the entry point, not a reusable module
                if module_path != "main" {
                    if let Ok(source) = fs::read_to_string(&path) {
                        modules.push((module_path, source));
                    }
                }
            }
        }
    }

    modules
}

/// Convert a file path to a module path string.
fn path_to_module_name(path: &Path, src_root: &Path) -> Option<String> {
    let relative = path.strip_prefix(src_root).ok()?;
    let mut segments = Vec::new();

    for component in relative.components() {
        if let std::path::Component::Normal(s) = component {
            let name = s.to_str()?;
            let name = name.strip_suffix(".ab").unwrap_or(name);
            segments.push(name);
        }
    }

    Some(segments.join("."))
}

/// Compile core library modules and load them into the VM.
///
/// This allows calling functions like `core.list.last([1, 2])` from the REPL.
fn compile_and_load_core_library(ctx: &mut ReplContext, vm: &mut Vm) {
    use ambient_engine::compiler::compile_module;
    use ambient_engine::core_library::CoreLibrary;

    for module_name in CoreLibrary::available_modules() {
        let Ok(source) = CoreLibrary::get_source(&[std::sync::Arc::from(module_name)]) else {
            continue;
        };

        // Parse the module source
        let Ok(module) = ambient_parser::parse(source) else {
            continue;
        };

        // Compile the module
        let Ok(compiled) = compile_module(&module) else {
            continue;
        };

        // Register each function with its qualified name and load into VM
        for (name, hash) in &compiled.function_names {
            let qualified_name: std::sync::Arc<str> = format!("core.{module_name}.{name}").into();

            // Register the hash so the expression compiler can find it
            ctx.register_core_function(qualified_name, *hash);
        }

        // Load all compiled functions into the VM
        for func in compiled.functions.into_values() {
            vm.load_function(func);
        }
    }
}

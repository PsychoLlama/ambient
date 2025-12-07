//! REPL (Read-Eval-Print-Loop) implementation.
//!
//! This module provides an interactive environment for evaluating Ambient code.

mod highlighter;

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::{Config as RustylineConfig, Editor};

use ambient_engine::abilities::register_all_standard_abilities;
use ambient_engine::format::format_value_colored;
use ambient_engine::compiler::{
    compile_expression_with_context, compile_repl_item, ReplContext, ReplItemKind,
};
use ambient_engine::vm::Vm;
use ambient_parser::ReplInput;

use highlighter::AmbientHelper;

/// Run the interactive REPL.
pub fn cmd_repl() -> Result<()> {
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

/// Get the history file path.
fn get_history_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("ambient").join("repl_history"))
}

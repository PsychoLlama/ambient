//! REPL (Read-Eval-Print-Loop) implementation.
//!
//! The REPL is a thin frontend over the same pipeline that powers `ambient
//! check`, the LSP, and `ambient run`. Each session is modeled as an
//! accumulating in-memory module named `repl`: every turn re-checks and
//! re-compiles the committed definitions (plus the new input) through
//! `ambient_analysis`/`ambient_engine`, so the REPL gets the full language —
//! type checking, every item kind, cross-module `use`, real shared
//! diagnostics, and the full platform ability set — without a bespoke
//! parallel compiler. "What is an error" and "how to compile a module set"
//! live in the shared layer, never here (see AGENTS.md).
//!
//! Execution-wise the REPL is a **deploy frontend** (see
//! `ref/live-upgrade.md`, "Generations"): the session holds one running
//! [`RuntimeHost`], and every turn — definition or expression — applies a
//! generation through [`RuntimeHost::deploy_incremental`]: load, validate,
//! swap the name table, run a synthetic entry as the reconciliation body.
//! A definition turn's entry is a no-op (the deploy *is* the point: the
//! swap rebinds the redefined name, and a running program picks it up at
//! its late-bound points — a task's next pass, a `Live::latest!` read); an
//! expression turn's entry is the expression. A rejected deploy (a failed
//! migration check) errors the turn with nothing committed and the running
//! program untouched. The generation re-ships the whole session build each
//! turn; content addressing makes the diff exact, so "one item plus
//! dependents" falls out as everything else reports `unchanged`.
//!
//! Turns deploy *incrementally*: one turn is never a full declaration of
//! the running program, so nothing is stopped or drained for being absent
//! from it — tasks ensured three turns ago keep running until an explicit
//! `Task::drain!` (or `:clear`, which winds the whole program down).

mod completer;
mod editor;
mod highlighter;
pub mod session;

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::{Config as RustylineConfig, Editor, EventHandler, KeyEvent};

use editor::ExternalEditorHandler;

use ambient_engine::format::format_value_colored;

use session::{ReplCommand, ReplIo, ReplSession, parse_repl_command};

/// Run the interactive REPL.
pub fn cmd_repl(project_dir: Option<&Path>) -> Result<()> {
    eprintln!("Type :help for commands.");

    // Determine project directory (default to current directory).
    let project_dir = match project_dir {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    let mut session = ReplSession::new(&project_dir, ReplIo::terminal())?;

    // Create the REPL helper (syntax highlighting).
    let completer = completer::ReplHelper::new();

    // Configure rustyline with our helper.
    let config = RustylineConfig::builder()
        .auto_add_history(true)
        .max_history_size(1000)
        .expect("valid history size")
        .build();

    let mut rl: Editor<completer::ReplHelper, DefaultHistory> =
        Editor::with_config(config).context("failed to initialize readline")?;
    rl.set_helper(Some(completer));

    // Bind Ctrl+E and Ctrl+O to open external editor (like bash's edit-and-execute-command).
    rl.bind_sequence(
        KeyEvent::ctrl('E'),
        EventHandler::Conditional(Box::new(ExternalEditorHandler)),
    );
    rl.bind_sequence(
        KeyEvent::ctrl('O'),
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
                    match parse_repl_command(line) {
                        ReplCommand::Quit => break,
                        other => {
                            if let Err(e) = session.run_command(other) {
                                eprintln!("\x1b[1;31merror\x1b[0m: {e}");
                            }
                        }
                    }
                    continue;
                }

                // Parse and evaluate the input.
                match session.eval(line) {
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

/// Get the history file path.
fn get_history_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("ambient").join("repl_history"))
}

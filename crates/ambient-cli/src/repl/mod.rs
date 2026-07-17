//! REPL (Read-Eval-Print-Loop) implementation.
//!
//! The REPL is an **exploration surface**: it evaluates expressions, saves
//! bindings (`x = expr`), imports names (`use …`), and inspects modules and
//! signatures. It does not author code — definitions live in module files,
//! and the session stays fresh the other way around: a background watcher
//! marks the session stale when a project source changes, and the next
//! prompt interaction rebuilds the base from disk and redeploys it
//! (`:reload` forces the same thing). See [`session`] for the session
//! model, scope anchoring, and binding semantics.
//!
//! It is a thin frontend over the same pipeline that powers `ambient
//! check`, the LSP, and `ambient run`: every turn re-checks and re-compiles
//! the session module through `ambient_analysis`/`ambient_engine`, so the
//! REPL gets real diagnostics and the full platform ability set without a
//! bespoke parallel compiler. "What is an error" and "how to compile a
//! module set" live in the shared layer, never here (see AGENTS.md).
//!
//! Execution-wise the REPL is a **deploy frontend** (see
//! `ref/live-upgrade.md`, "Generations"): the session holds one running
//! [`RuntimeHost`], and every expression turn applies a generation through
//! [`RuntimeHost::deploy_incremental`] whose synthetic entry is the
//! expression (called with the saved bindings as arguments). A rejected
//! deploy (a failed migration check) errors the turn with the running
//! program untouched. Turns deploy *incrementally*: one turn is never a
//! full declaration of the running program, so nothing is drained for
//! being absent from it — tasks ensured three turns ago keep running until
//! an explicit `Task::drain!` (or `:clear`, which winds the program down).

mod base;
pub mod completer;
mod editor;
mod highlighter;
mod inspect;
mod render;
pub mod session;

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::{Config as RustylineConfig, Editor, EventHandler, ExternalPrinter, KeyEvent};

use crate::commands::watch;
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

    // Create the REPL helper (tab completion + syntax highlighting). It
    // shares a completion snapshot with the loop below, which refreshes it
    // after every handled line — imports, bindings, and (after a reload)
    // the registry may all have changed.
    let snapshot = std::sync::Arc::new(std::sync::Mutex::new(session.completion_snapshot()));
    let completer = completer::ReplHelper::new(std::sync::Arc::clone(&snapshot));

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

    // Watch project sources so the session stays fresh: a change marks it
    // stale (announced once, prompt-safely, via the external printer) and
    // the next prompt interaction reloads before evaluating. Watch errors
    // degrade to `:reload`-only, they don't block the REPL.
    let watcher = session.project_root().and_then(|root| {
        let printer = rl.create_external_printer().ok();
        let printer = std::sync::Mutex::new(printer);
        match watch::SourceWatcher::spawn(root, move || {
            if let Ok(mut printer) = printer.lock()
                && let Some(printer) = printer.as_mut()
            {
                let _ = printer.print(
                    "\x1b[2msources changed; reloading at the next input\x1b[0m".to_string(),
                );
            }
        }) {
            Ok(watcher) => Some(watcher),
            Err(e) => {
                eprintln!("warning: source watching disabled: {e}");
                None
            }
        }
    });

    // Refresh the shared completion snapshot from the session's current
    // state (called after anything that may change imports, bindings, or
    // the registry).
    let refresh_completions = |session: &ReplSession| {
        if let Ok(mut snap) = snapshot.lock() {
            *snap = session.completion_snapshot();
        }
    };

    loop {
        // Flush stdout before reading (in case any output is buffered).
        let _ = io::stdout().flush();

        let readline = rl.readline("> ");
        match readline {
            Ok(line) => {
                let line = line.trim();

                // Sources changed while we sat at the prompt: refresh the
                // base before doing anything with this input, so the turn
                // always sees the code that is on disk.
                if watcher
                    .as_ref()
                    .is_some_and(watch::SourceWatcher::take_dirty)
                    && let Err(e) = session.reload()
                {
                    eprintln!("\x1b[1;31merror\x1b[0m: {e}");
                }

                // Skip empty lines (a dirty reload above may still have
                // changed the session, so keep completion fresh).
                if line.is_empty() {
                    refresh_completions(&session);
                    continue;
                }

                // Handle REPL commands. A leading `::` is a workspace-rooted
                // path (`::lib::greet()`), not a command.
                if line.starts_with(':') && !line.starts_with("::") {
                    match parse_repl_command(line) {
                        ReplCommand::Quit => break,
                        other => {
                            if let Err(e) = session.run_command(other) {
                                eprintln!("\x1b[1;31merror\x1b[0m: {e}");
                            }
                        }
                    }
                    refresh_completions(&session);
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
                refresh_completions(&session);
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

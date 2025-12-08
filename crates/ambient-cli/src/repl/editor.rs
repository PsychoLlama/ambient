//! External editor support for the REPL.
//!
//! This module provides functionality to edit the current REPL line in an external editor.

use std::env;
use std::fs;
use std::io;
use std::process::Command;

use rustyline::{Cmd, ConditionalEventHandler, Event, EventContext, RepeatCount};

/// Open the given text in the user's preferred editor and return the edited content.
///
/// Uses $EDITOR, $VISUAL, or falls back to "vi".
pub fn edit_in_external_editor(initial_content: &str) -> io::Result<String> {
    // Create a temporary file with the content.
    let temp_dir = env::temp_dir();
    let temp_file = temp_dir.join(format!("ambient_repl_{}.amb", std::process::id()));

    fs::write(&temp_file, initial_content)?;

    // Get the editor from environment.
    let editor = env::var("EDITOR")
        .or_else(|_| env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());

    // Run the editor.
    let status = Command::new(&editor).arg(&temp_file).status()?;

    if !status.success() {
        fs::remove_file(&temp_file).ok();
        return Err(io::Error::other(format!(
            "Editor exited with status: {}",
            status
        )));
    }

    // Read the edited content.
    let content = fs::read_to_string(&temp_file)?;

    // Clean up.
    fs::remove_file(&temp_file).ok();

    // Trim trailing newlines but preserve the content.
    Ok(content.trim_end().to_string())
}

/// Event handler for opening the external editor (Ctrl+E).
///
/// This handler captures the current line content and opens it in an external editor.
/// When the editor exits, the content replaces the current line.
pub struct ExternalEditorHandler;

impl ConditionalEventHandler for ExternalEditorHandler {
    fn handle(
        &self,
        _evt: &Event,
        _n: RepeatCount,
        _positive: bool,
        ctx: &EventContext<'_>,
    ) -> Option<Cmd> {
        let current_line = ctx.line();

        // Edit in external editor.
        match edit_in_external_editor(current_line) {
            Ok(new_content) => {
                // Replace the entire line with the new content.
                Some(Cmd::Replace(
                    rustyline::Movement::WholeBuffer,
                    Some(new_content),
                ))
            }
            Err(e) => {
                // Print error and return to normal editing.
                eprintln!("\nEditor error: {e}");
                // Repaint the prompt.
                Some(Cmd::Repaint)
            }
        }
    }
}

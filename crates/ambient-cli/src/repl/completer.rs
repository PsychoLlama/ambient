//! Tab completion and hint support for the REPL.
//!
//! Provides:
//! - Ghost text hints showing the best completion
//! - Tab cycling through completion candidates
//! - Completion for keywords, types, abilities, modules, and user-defined symbols
//!
//! This module delegates to the LSP completion service via `ReplLspBridge`
//! to avoid duplicating completion logic.

use std::borrow::Cow;
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};

use ambient_engine::compiler::ReplContext;

use super::highlighter::AmbientHighlighter;
use super::lsp_bridge::ReplLspBridge;

/// REPL completer with tab-cycling and ghost text hints.
///
/// Uses the LSP completion service for completions, wrapped in `ReplLspBridge`.
pub struct ReplCompleter {
    /// LSP bridge for completions.
    /// RefCell allows interior mutability needed because rustyline's Completer
    /// trait takes &self, not &mut self.
    bridge: RefCell<ReplLspBridge>,
    /// Shared REPL context for user-defined symbols.
    repl_ctx: Arc<Mutex<ReplContext>>,
    /// Syntax highlighter.
    highlighter: AmbientHighlighter,
}

impl ReplCompleter {
    /// Create a new completer for the given project directory.
    pub fn new(project_dir: PathBuf, repl_ctx: Arc<Mutex<ReplContext>>) -> Self {
        Self {
            bridge: RefCell::new(ReplLspBridge::new(&project_dir)),
            repl_ctx,
            highlighter: AmbientHighlighter,
        }
    }
}

impl Helper for ReplCompleter {}

impl Completer for ReplCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        // Find the word being typed for replacement position
        let before_cursor = &line[..pos];
        let word_start = before_cursor
            .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .map_or(0, |i| i + 1);

        // Sync REPL context and get completions from LSP bridge
        let ctx = self.repl_ctx.lock().unwrap();
        let mut bridge = self.bridge.borrow_mut();
        bridge.sync_repl_context(&ctx);
        drop(ctx);

        let completions = bridge.get_completions(line, pos);

        let pairs: Vec<Pair> = completions
            .into_iter()
            .map(|c| Pair {
                display: c.label,
                replacement: c.replacement,
            })
            .collect();

        Ok((word_start, pairs))
    }
}

impl Hinter for ReplCompleter {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<String> {
        // Only show hint if cursor is at the end of the line
        if pos != line.len() {
            return None;
        }

        // Find the word being typed
        let word_start = line
            .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .map_or(0, |i| i + 1);
        let prefix = &line[word_start..];

        // Need at least one character to show a hint
        if prefix.is_empty() {
            return None;
        }

        // Sync REPL context and get hint from LSP bridge
        let ctx = self.repl_ctx.lock().unwrap();
        let mut bridge = self.bridge.borrow_mut();
        bridge.sync_repl_context(&ctx);
        drop(ctx);

        bridge.get_hint(line, pos).map(|suffix| {
            // Return with dim gray color
            format!("\x1b[90m{suffix}\x1b[0m")
        })
    }
}

impl Validator for ReplCompleter {}

impl Highlighter for ReplCompleter {
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

    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        // Hint is already colored in the hint() method
        Cow::Borrowed(hint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a test completer with no project.
    fn test_completer() -> ReplCompleter {
        ReplCompleter::new(
            PathBuf::from("/nonexistent"),
            Arc::new(Mutex::new(ReplContext::new())),
        )
    }

    #[test]
    fn test_completer_empty_input() {
        let completer = test_completer();
        let history = rustyline::history::DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);

        let result = completer.complete("", 0, &ctx);
        assert!(result.is_ok());
        let (_, pairs) = result.unwrap();

        // Should have many completions (keywords, types, abilities)
        assert!(
            pairs.len() > 10,
            "Expected many completions, got {}",
            pairs.len()
        );
    }

    #[test]
    fn test_completer_console() {
        let completer = test_completer();
        let history = rustyline::history::DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);

        let result = completer.complete("Con", 3, &ctx);
        assert!(result.is_ok());
        let (_, pairs) = result.unwrap();

        // Should include Console
        assert!(
            pairs.iter().any(|p| p.replacement == "Console"),
            "Should complete Console, got: {:?}",
            pairs.iter().map(|p| &p.replacement).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_completer_console_dot() {
        let completer = test_completer();
        let history = rustyline::history::DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);

        let result = completer.complete("Console.", 8, &ctx);
        assert!(result.is_ok());
        let (_, pairs) = result.unwrap();

        // Should show Console methods
        let methods: Vec<_> = pairs.iter().map(|p| p.replacement.as_str()).collect();
        assert!(
            methods.iter().any(|m| m.contains("print")),
            "Should show print method, got: {:?}",
            methods
        );
    }

    #[test]
    fn test_completer_keyword() {
        let completer = test_completer();
        let history = rustyline::history::DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);

        let result = completer.complete("le", 2, &ctx);
        assert!(result.is_ok());
        let (_, pairs) = result.unwrap();

        assert!(
            pairs.iter().any(|p| p.replacement == "let"),
            "Should complete let"
        );
    }

    #[test]
    fn test_completer_repl_symbols() {
        let repl_ctx = Arc::new(Mutex::new(ReplContext::new()));
        {
            let mut ctx = repl_ctx.lock().unwrap();
            ctx.register_function(Arc::from("my_function"), blake3::hash(b"test"));
            ctx.register_constant(Arc::from("MY_CONST"), blake3::hash(b"test2"));
        }
        let completer = ReplCompleter::new(PathBuf::from("/nonexistent"), repl_ctx);

        let history = rustyline::history::DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);

        let result = completer.complete("my", 2, &ctx);
        assert!(result.is_ok());
        let (_, pairs) = result.unwrap();

        assert!(
            pairs.iter().any(|p| p.replacement == "my_function"),
            "Should include REPL-defined function"
        );
    }

    #[test]
    fn test_hinter_basic() {
        let completer = test_completer();
        let history = rustyline::history::DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);

        // Hint for "Con" should suggest "sole" (completing to Console)
        let hint = completer.hint("Con", 3, &ctx);
        assert!(hint.is_some(), "Should provide hint for Con");

        // The hint should contain "sole" (the suffix to complete Console)
        let hint_text = hint.unwrap();
        assert!(
            hint_text.contains("sole"),
            "Hint should be 'sole', got: {}",
            hint_text
        );
    }

    #[test]
    fn test_hinter_empty() {
        let completer = test_completer();
        let history = rustyline::history::DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);

        // No hint for empty input
        let hint = completer.hint("", 0, &ctx);
        assert!(hint.is_none());
    }

    #[test]
    fn test_hinter_cursor_not_at_end() {
        let completer = test_completer();
        let history = rustyline::history::DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);

        // No hint when cursor is not at end
        let hint = completer.hint("Console", 3, &ctx);
        assert!(hint.is_none());
    }
}

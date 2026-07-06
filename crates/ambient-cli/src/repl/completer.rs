//! Syntax highlighting support for the REPL.
//!
//! Provides the rustyline `Helper` used by the REPL. It implements syntax
//! highlighting only; completion and hinting are intentionally no-ops.
//!
//! rustyline requires a `Helper` to implement `Completer`, `Hinter`,
//! `Validator`, and `Highlighter`, so all four are present, but only
//! `Highlighter` does any work.

use std::borrow::Cow;

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};

use super::highlighter::AmbientHighlighter;

/// REPL helper providing syntax highlighting.
pub struct ReplHelper {
    /// Syntax highlighter.
    highlighter: AmbientHighlighter,
}

impl ReplHelper {
    /// Create a new REPL helper.
    pub fn new() -> Self {
        Self {
            highlighter: AmbientHighlighter,
        }
    }
}

impl Helper for ReplHelper {}

impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        _line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        Ok((pos, Vec::new()))
    }
}

impl Hinter for ReplHelper {
    type Hint = String;

    fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<String> {
        None
    }
}

impl Validator for ReplHelper {}

impl Highlighter for ReplHelper {
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
        Cow::Borrowed(hint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_helper_constructs() {
        let helper = ReplHelper::new();
        let history = rustyline::history::DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);

        // Completion is a no-op now.
        let (_, pairs) = helper.complete("platform::St", 12, &ctx).unwrap();
        assert!(pairs.is_empty());

        // Hinting is a no-op now.
        assert!(helper.hint("platform::St", 12, &ctx).is_none());
    }
}

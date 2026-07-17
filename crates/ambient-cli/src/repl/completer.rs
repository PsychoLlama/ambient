//! Tab completion and syntax highlighting for the REPL.
//!
//! Provides the rustyline `Helper` used by the REPL. Completion runs the
//! same frontend-neutral pipeline as the LSP
//! (`ambient_analysis::completions`): the in-progress line is wrapped in
//! the session's trial-module shape (committed `use` imports + a synthetic
//! entry function whose parameters are the saved bindings — see
//! [`CompletionSnapshot`]), the cursor is mapped into it, and the shared
//! `get_completions` answers from the checked module. Lines starting with
//! `:` complete the REPL's own commands instead.
//!
//! rustyline requires a `Helper` to implement `Completer`, `Hinter`,
//! `Validator`, and `Highlighter`, so all four are present; hinting stays
//! a no-op.

use std::borrow::Cow;
use std::sync::{Arc, Mutex};

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};

use ambient_analysis::completions::{CompletionContext, CompletionItem, get_completions};
use ambient_engine::ability_resolver::AbilityResolver;

use super::highlighter::AmbientHighlighter;
use super::session::CompletionSnapshot;

/// The REPL's `:command` names, as completion offers them (aliases with a
/// single letter aren't worth completing). Mirrors `parse_repl_command`.
const REPL_COMMANDS: &[&str] = &[
    "help",
    "quit",
    "clear",
    "reset",
    "reload",
    "sig",
    "signature",
    "type",
];

/// REPL helper providing tab completion and syntax highlighting.
pub struct ReplHelper {
    /// Syntax highlighter.
    highlighter: AmbientHighlighter,
    /// The session's completion inputs, refreshed by the REPL loop after
    /// every evaluated line (imports/bindings may have changed).
    snapshot: Arc<Mutex<CompletionSnapshot>>,
    /// The ability resolver completion contexts classify path heads with —
    /// the same platform-prelude resolver the LSP uses. Built once; it
    /// never changes across turns.
    resolver: AbilityResolver,
}

impl ReplHelper {
    /// Create a new REPL helper over a shared completion snapshot.
    pub fn new(snapshot: Arc<Mutex<CompletionSnapshot>>) -> Self {
        Self {
            highlighter: AmbientHighlighter,
            snapshot,
            resolver: ambient_analysis::platform_prelude_resolver(),
        }
    }
}

impl Helper for ReplHelper {}

impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        if let Some(result) = complete_command(line, pos) {
            return Ok(result);
        }
        let Ok(snapshot) = self.snapshot.lock() else {
            return Ok((pos, Vec::new()));
        };
        let (start, items) = completions_for_line(&snapshot, &self.resolver, line, pos);
        Ok((start, items.iter().map(pair_for).collect()))
    }
}

/// Complete the leading `:command` word of a REPL command line. `None` when
/// the line is not a command, or the cursor sits past the command word (a
/// `:sig`/`:type` argument falls through to the ordinary pipeline, which
/// completes paths like `core::opt`).
fn complete_command(line: &str, pos: usize) -> Option<(usize, Vec<Pair>)> {
    let rest = line.strip_prefix(':')?;
    let word = &rest[..rest.len().min(pos.saturating_sub(1))];
    if pos == 0 || word.contains(char::is_whitespace) {
        return None;
    }
    let pairs = REPL_COMMANDS
        .iter()
        .filter(|cmd| cmd.starts_with(word))
        .map(|cmd| Pair {
            display: format!(":{cmd}"),
            replacement: (*cmd).to_string(),
        })
        .collect();
    // Replace from just after the `:`.
    Some((1, pairs))
}

/// Run the shared completion pipeline over the line at `pos`: wrap it in
/// the session's trial-module shape, map the cursor, check, and complete.
/// Returns the byte offset in `line` where the completed word starts,
/// plus the neutral items. Public so integration tests can drive the real
/// completion path over a live session's snapshot.
#[must_use]
pub fn completions_for_line(
    snapshot: &CompletionSnapshot,
    resolver: &AbilityResolver,
    line: &str,
    pos: usize,
) -> (usize, Vec<CompletionItem>) {
    let pos = pos.min(line.len());
    let trial = format!("{}{line}\n}}\n", snapshot.prefix);
    let cursor = snapshot.prefix.len() + pos;

    let ctx = CompletionContext::new(&trial, cursor, resolver);
    // The word being completed lives entirely in `line` (the prefix ends at
    // a newline), so its start maps back by the same shift.
    let start = pos - ctx.word_prefix.len();

    // The typed module the pipeline answers from: the healed check when the
    // trial parses cleanly, else the recovering parse's partial module —
    // exactly what the LSP holds mid-keystroke — leaving the pipeline's own
    // completion-time healing to cover the broken-parse cases (dangling
    // `.`, partial impl members).
    let module = ambient_analysis::healed_module_for_completion(
        &trial,
        &snapshot.session_path,
        &snapshot.registry,
    )
    .unwrap_or_else(|| {
        ambient_analysis::check_without_cycle(
            &trial,
            Some(&snapshot.session_path),
            Some(&snapshot.registry),
            None,
        )
        .module
    });

    let items = get_completions(
        &ctx,
        &trial,
        Some(&module),
        Some(&snapshot.session_path),
        Some(&snapshot.registry),
        resolver,
    );
    (start, items)
}

/// Render a neutral completion item as a rustyline candidate: the label
/// (with its one-line detail for context) displays; the label inserts.
fn pair_for(item: &CompletionItem) -> Pair {
    let display = match &item.detail {
        Some(detail) if *detail != item.label => format!("{} \x1b[2m{detail}\x1b[0m", item.label),
        _ => item.label.clone(),
    };
    Pair {
        display,
        replacement: item.label.clone(),
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

    /// A snapshot shaped like a fresh session outside a project: no imports,
    /// no bindings, the bare `__repl` module against core + platform.
    fn bare_snapshot() -> CompletionSnapshot {
        let registry = ambient_analysis::core_platform_registry();
        CompletionSnapshot {
            prefix: "fn __repl_complete() {\n".to_string(),
            registry,
            session_path: ambient_engine::module_path::ModulePath::from_str_segments(&["__repl"])
                .unwrap(),
        }
    }

    fn complete_bare(line: &str, pos: usize) -> (usize, Vec<CompletionItem>) {
        let resolver = ambient_analysis::platform_prelude_resolver();
        completions_for_line(&bare_snapshot(), &resolver, line, pos)
    }

    #[test]
    fn completes_keywords_and_word_start() {
        let (start, items) = complete_bare("le", 2);
        assert_eq!(start, 0);
        assert!(items.iter().any(|i| i.label == "let"));
    }

    #[test]
    fn completes_core_paths_with_qualifier_start() {
        let (start, items) = complete_bare("core::opt", 9);
        // Only the final segment is replaced.
        assert_eq!(start, 6);
        assert!(items.iter().any(|i| i.label == "option"));
    }

    #[test]
    fn completes_ability_methods() {
        let (start, items) = complete_bare("Stdio::o", 8);
        assert_eq!(start, 7);
        assert!(items.iter().any(|i| i.label == "out!"));
    }

    #[test]
    fn completes_dot_members_on_typed_expression() {
        // The dangling dot exercises the pipeline's completion-time healing
        // through the REPL's trial wrapper.
        let line = "\"hi\".";
        let (start, items) = complete_bare(line, line.len());
        assert_eq!(start, line.len());
        assert!(
            items.iter().any(|i| i.label == "length"),
            "String members should complete: {:?}",
            items.iter().map(|i| &i.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn completes_binding_params_as_locals() {
        // Bindings are entry params in the trial source; annotated with
        // their recorded type they complete as typed locals.
        let snapshot = CompletionSnapshot {
            prefix: "fn __repl_complete(counter: Number) {\n".to_string(),
            ..bare_snapshot()
        };
        let resolver = ambient_analysis::platform_prelude_resolver();
        let (start, items) = completions_for_line(&snapshot, &resolver, "cou", 3);
        assert_eq!(start, 0);
        let item = items
            .iter()
            .find(|i| i.label == "counter")
            .expect("binding should complete");
        assert_eq!(item.detail.as_deref(), Some("counter: Number"));
    }

    #[test]
    fn completes_commands_only_in_command_word() {
        let (start, pairs) = complete_command(":he", 3).unwrap();
        assert_eq!(start, 1);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].replacement, "help");

        // Empty command word offers everything.
        let (_, pairs) = complete_command(":", 1).unwrap();
        assert_eq!(pairs.len(), REPL_COMMANDS.len());

        // Past the command word, fall through to the ordinary pipeline.
        assert!(complete_command(":sig core::opt", 14).is_none());
        // Not a command line at all.
        assert!(complete_command("1 + 2", 5).is_none());
    }

    #[test]
    fn sig_argument_paths_complete_through_pipeline() {
        // `:sig core::opt` falls through command completion; the pipeline
        // still completes the path because core:: completion is text-driven.
        let line = ":sig core::opt";
        let (start, items) = complete_bare(line, line.len());
        assert_eq!(start, line.len() - "opt".len());
        assert!(items.iter().any(|i| i.label == "option"));
    }
}

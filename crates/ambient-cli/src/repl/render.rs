//! The REPL's prompt-facing text: command parsing, help, and error
//! rendering. Split from [`session`](super::session) to keep both under the
//! per-file line budget; the session re-exports the public pieces so
//! frontends and the test harness keep addressing them as
//! `repl::session::…`.

use ambient_analysis::Diagnostic;
use ambient_engine::ast::{Item, ItemKind};

use super::session::ENTRY_PREFIX;

/// REPL command types.
pub enum ReplCommand {
    Help,
    Quit,
    Clear,
    Reload,
    /// `:sig <path>` — show a member's signature and doc.
    Sig(String),
    /// `:type <expr>` — show an expression's inferred type without running it.
    Type(String),
    Unknown(String),
}

/// Parse a REPL command line (leading `:` already present).
#[must_use]
pub fn parse_repl_command(line: &str) -> ReplCommand {
    let line = line.trim_start_matches(':');
    let (cmd, rest) = match line.split_once(char::is_whitespace) {
        Some((cmd, rest)) => (cmd, rest.trim()),
        None => (line, ""),
    };
    match cmd {
        "help" | "h" | "?" => ReplCommand::Help,
        "quit" | "q" | "exit" => ReplCommand::Quit,
        "clear" | "reset" => ReplCommand::Clear,
        "reload" | "r" => ReplCommand::Reload,
        "sig" | "signature" | "s" => ReplCommand::Sig(rest.to_string()),
        "type" | "t" => ReplCommand::Type(rest.to_string()),
        other => ReplCommand::Unknown(other.to_string()),
    }
}

/// Write the REPL help text line by line to `emit`. Shared by both frontends
/// so the help content lives in exactly one place.
pub fn write_repl_help(emit: &dyn Fn(&str)) {
    emit("REPL Commands:");
    emit("  :help, :h, :?    Show this help message");
    emit("  :quit, :q, :exit Exit the REPL");
    emit("  :clear, :reset   Clear imports, bindings, and the running program");
    emit("  :reload, :r      Rebuild the project from disk and redeploy");
    emit("");
    emit("Key Bindings:");
    emit("  Ctrl+E, Ctrl+O   Edit current line in $EDITOR");
    emit("");
    emit("The REPL is for exploring code, not authoring it:");
    emit("  1 + 2 * 3            Evaluate an expression");
    emit("  parse(\"[1, 2]\")      Call project and core functions");
    emit("  xs = List::of(1, 2)  Save a value; later turns see `xs`");
    emit("  use pkg::utils;      Import for the rest of the session");
    emit("  core::option         Inspect a module");
    emit("");
    emit("Definitions (fn/struct/trait/ability/impl/…) live in module files.");
    emit("Saved file changes flow into the session (`:reload` forces it);");
    emit("tasks pick rebound names up on their next pass.");
    emit("");
    emit("The session's scope is the directory the REPL was started in:");
    emit("`pkg`, `self`, and `super` resolve as they would in a file there.");
}

/// The error for a definition entered at the prompt: the REPL is an
/// exploration surface, not an authoring one.
pub(crate) fn definitions_unsupported(item: &Item) -> String {
    let what = match &item.kind {
        ItemKind::Function(_) => "a function",
        ItemKind::Const(_) => "a constant",
        ItemKind::Struct(_) => "a struct",
        ItemKind::TypeAlias(_) => "a type alias",
        ItemKind::Enum(_) => "an enum",
        ItemKind::Ability(_) => "an ability",
        ItemKind::Trait(_) => "a trait",
        ItemKind::ExternFn(_) => "an extern fn",
        ItemKind::Set(_) => "an ability set",
        ItemKind::Impl(_) => "an impl",
        ItemKind::Use(_) => "an import",
    };
    format!(
        "the REPL doesn't host definitions: write {what} in a module file \
         instead — the session picks up saved changes (`:reload` forces it).\n\
         The REPL evaluates expressions, saves bindings (`x = expr`), and \
         imports names (`use …`)."
    )
}

/// Whether the input references any of the project-relative path roots.
pub(crate) fn mentions_project_roots(line: &str) -> bool {
    ["pkg", "self", "super"].iter().any(|root| {
        line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .any(|word| word == *root)
    })
}

/// Rewrite the synthetic entry name (`__repl_entry_7`) out of rendered
/// errors: the wrapper is an implementation detail, and a note like
/// ``in function `__repl_entry_7` `` should read as "in this input".
pub(crate) fn scrub_entry_names(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find(ENTRY_PREFIX) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + ENTRY_PREFIX.len()..];
        let digits = after.chars().take_while(char::is_ascii_digit).count();
        out.push_str("repl input");
        rest = &after[digits..];
    }
    out.push_str(rest);
    out
}

/// Format a parse error for REPL display (without file path).
pub(crate) fn format_repl_parse_error(line: &str, error: &ambient_parser::ParseError) -> String {
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

/// Render shared analysis diagnostics against the trial module source.
///
/// The trial source is `committed imports + this turn's input`, so a caret
/// can point at the exact offending line even when the error is in the
/// synthetic entry wrapper.
pub(crate) fn format_repl_diagnostics(source: &str, diagnostics: &[Diagnostic]) -> String {
    let mut out = String::new();
    for (i, diag) in diagnostics.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let (col, line_start, line_end) = line_info(source, diag.span.start as usize);
        let line_content = &source[line_start..line_end];

        out.push_str(&diag.message);
        out.push('\n');
        out.push_str(&format!("  {line_content}\n"));

        let underline_start = col;
        let underline_len = ((diag.span.end - diag.span.start) as usize)
            .min(line_content.len().saturating_sub(underline_start))
            .max(1);
        out.push_str(&format!(
            "  {}{}",
            " ".repeat(underline_start),
            "^".repeat(underline_len)
        ));

        if let Some(note) = &diag.note {
            out.push_str(&format!("\n  note: {note}"));
        }
    }
    out
}

/// Column offset within its line and the line's byte bounds for `offset`.
fn line_info(source: &str, offset: usize) -> (usize, usize, usize) {
    let mut line_start = 0;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line_start = i + 1;
        }
    }
    let line_end = source[line_start..]
        .find('\n')
        .map_or(source.len(), |i| line_start + i);
    (offset.saturating_sub(line_start), line_start, line_end)
}

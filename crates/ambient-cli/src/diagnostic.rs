//! Unified diagnostic formatting for compiler errors.
//!
//! This module provides a common abstraction for formatting parse errors,
//! type errors, and other diagnostics with source context.

use std::path::Path;

use ambient_engine::build::BuildError;

/// A diagnostic that can be formatted with source context.
pub trait Diagnostic {
    /// The error message to display.
    fn message(&self) -> String;

    /// The byte offset range in the source.
    fn span(&self) -> (u32, u32);

    /// Optional context/note to display after the error.
    fn context(&self) -> Option<&str>;
}

impl Diagnostic for ambient_parser::ParseError {
    fn message(&self) -> String {
        self.kind.to_string()
    }

    fn span(&self) -> (u32, u32) {
        (self.span.start, self.span.end)
    }

    fn context(&self) -> Option<&str> {
        self.context.as_deref()
    }
}

impl Diagnostic for ambient_analysis::Diagnostic {
    fn message(&self) -> String {
        self.message.clone()
    }

    fn span(&self) -> (u32, u32) {
        (self.span.start, self.span.end)
    }

    fn context(&self) -> Option<&str> {
        self.note.as_deref()
    }
}

/// ANSI color codes for error formatting.
mod colors {
    pub const RED_BOLD: &str = "\x1b[1;31m";
    pub const BLUE_BOLD: &str = "\x1b[1;34m";
    pub const RESET: &str = "\x1b[0m";
}

/// Print a diagnostic with source context.
pub fn print_diagnostic<D: Diagnostic>(source: &str, file: &Path, error: &D) {
    let (start, end) = error.span();
    let (line_num, col, line_start, line_end) = find_line_info(source, start as usize);
    let line_content = &source[line_start..line_end];

    // Error header
    eprintln!(
        "{RED}error{RESET}: {}",
        error.message(),
        RED = colors::RED_BOLD,
        RESET = colors::RESET
    );

    // Location
    eprintln!(
        "  {BLUE}-->{RESET} {}:{}:{}",
        file.display(),
        line_num,
        col,
        BLUE = colors::BLUE_BOLD,
        RESET = colors::RESET
    );

    // Source context
    let line_num_str = format!("{line_num}");
    let padding = " ".repeat(line_num_str.len());

    eprintln!(
        "   {padding} {BLUE}|{RESET}",
        BLUE = colors::BLUE_BOLD,
        RESET = colors::RESET
    );
    eprintln!(
        " {line_num_str} {BLUE}|{RESET} {line_content}",
        BLUE = colors::BLUE_BOLD,
        RESET = colors::RESET
    );

    // Error underline
    let underline_start = col.saturating_sub(1);
    let underline_len = ((end - start) as usize)
        .min(line_content.len().saturating_sub(underline_start))
        .max(1);
    let spaces = " ".repeat(underline_start);
    let carets = "^".repeat(underline_len);

    eprintln!(
        "   {padding} {BLUE}|{RESET} {spaces}{RED}{carets}{RESET}",
        BLUE = colors::BLUE_BOLD,
        RED = colors::RED_BOLD,
        RESET = colors::RESET
    );

    // Context/note if available
    if let Some(ctx) = error.context() {
        eprintln!(
            "   {padding} {BLUE}|{RESET}",
            BLUE = colors::BLUE_BOLD,
            RESET = colors::RESET
        );
        eprintln!(
            "   {padding} {BLUE}= note:{RESET} {ctx}",
            BLUE = colors::BLUE_BOLD,
            RESET = colors::RESET
        );
    }

    eprintln!();
}

/// Render a package [`BuildError`] and return a short summary error to
/// bail with.
///
/// Type and parse errors print byte-identically to `ambient check`: both
/// convert to `ambient_analysis::Diagnostic` — the one conversion the check
/// pipeline uses — and render through [`print_diagnostic`] against the
/// module's real source. Message-only failures (package open, codegen,
/// embedded modules) have no source to point at and surface as a plain
/// error.
pub fn report_build_error(error: BuildError) -> anyhow::Error {
    match error {
        BuildError::TypeCheck {
            path,
            source,
            errors,
            ..
        } => {
            let diagnostics = ambient_analysis::type_error_diagnostics(&errors);
            for diagnostic in &diagnostics {
                print_diagnostic(&source, &path, diagnostic);
            }
            anyhow::anyhow!(
                "Found {} type error(s) in {}",
                diagnostics.len(),
                path.display()
            )
        }
        BuildError::Parse {
            path,
            source,
            error,
            ..
        } => {
            let error = *error;
            let diagnostic = ambient_analysis::Diagnostic::error(
                ambient_engine::ast::Span::new(error.span.0, error.span.1),
                error.message,
                error.context,
            );
            print_diagnostic(&source, &path, &diagnostic);
            anyhow::anyhow!("parse error in {}", path.display())
        }
        BuildError::PackageOpen(msg) => anyhow::anyhow!("failed to open package: {msg}"),
        BuildError::Compile { module, error } => {
            anyhow::anyhow!("compile error in {module}: {error}")
        }
    }
}

/// Find line number, column, and line bounds for a byte offset.
fn find_line_info(source: &str, offset: usize) -> (usize, usize, usize, usize) {
    let mut line_num = 1;
    let mut line_start = 0;

    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line_num += 1;
            line_start = i + 1;
        }
    }

    let line_end = source[line_start..]
        .find('\n')
        .map_or(source.len(), |i| line_start + i);

    let col = offset - line_start + 1;
    (line_num, col, line_start, line_end)
}

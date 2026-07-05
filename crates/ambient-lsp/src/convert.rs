//! Conversion between shared analysis diagnostics and LSP types.
//!
//! The *content* of every diagnostic (span, message, note, and the policy
//! of what gets reported) comes from `ambient-analysis`, shared with
//! `ambient check`. This module only translates byte offsets to LSP
//! positions.

use lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

use crate::documents::Document;

/// Convert a shared analysis diagnostic to an LSP diagnostic.
#[must_use]
pub fn diagnostic_to_lsp(doc: &Document, diagnostic: &ambient_analysis::Diagnostic) -> Diagnostic {
    let severity = match diagnostic.severity {
        ambient_analysis::Severity::Error => DiagnosticSeverity::ERROR,
        ambient_analysis::Severity::Warning => DiagnosticSeverity::WARNING,
    };

    let mut message = diagnostic.message.clone();
    if let Some(note) = &diagnostic.note {
        message.push_str("\nnote: ");
        message.push_str(note);
    }

    Diagnostic {
        range: offset_range_to_lsp_range(
            doc,
            diagnostic.span.start as usize,
            diagnostic.span.end as usize,
        ),
        severity: Some(severity),
        source: Some("ambient".to_string()),
        message,
        ..Default::default()
    }
}

/// Convert a byte offset to an LSP position.
#[must_use]
pub fn offset_to_position(doc: &Document, offset: usize) -> Position {
    let (line, character) = doc.offset_to_position(offset);
    Position { line, character }
}

/// Convert a byte offset range to an LSP range.
#[must_use]
pub fn offset_range_to_lsp_range(doc: &Document, start: usize, end: usize) -> Range {
    Range {
        start: offset_to_position(doc, start),
        end: offset_to_position(doc, end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Uri;

    fn test_doc(content: &str) -> Document {
        let uri: Uri = "file:///test.ab".parse().expect("valid uri");
        Document::new(uri, 1, content.to_string())
    }

    #[test]
    fn test_offset_to_position_single_line() {
        let doc = test_doc("let x = 42");
        let pos = offset_to_position(&doc, 4);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 4);
    }

    #[test]
    fn test_offset_to_position_multiline() {
        let doc = test_doc("line1\nline2\nline3");
        // Beginning of line2 (after "line1\n" which is 6 chars)
        let pos = offset_to_position(&doc, 6);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 0);

        // Middle of line2
        let pos = offset_to_position(&doc, 9);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 3);
    }

    #[test]
    fn test_offset_range_to_lsp_range() {
        let doc = test_doc("fn foo() {\n  42\n}");
        let range = offset_range_to_lsp_range(&doc, 3, 6);
        // "foo" starts at offset 3 on line 0
        assert_eq!(range.start.line, 0);
        assert_eq!(range.start.character, 3);
        assert_eq!(range.end.line, 0);
        assert_eq!(range.end.character, 6);
    }

    #[test]
    fn diagnostics_carry_shared_message_text() {
        let source = "fn bad(): string { 42 }";
        let doc = test_doc(source);
        let result = ambient_analysis::analyze(source);
        let diagnostics = result.diagnostics();
        assert!(!diagnostics.is_empty());

        let lsp = diagnostic_to_lsp(&doc, &diagnostics[0]);
        // Identical text to what `ambient check` prints.
        assert!(lsp.message.starts_with(&diagnostics[0].message));
        assert_eq!(lsp.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(lsp.source.as_deref(), Some("ambient"));
    }

    #[test]
    fn parse_error_diagnostic_has_position() {
        let source = "fn foo(): number { 1 }\nfn broken(\n";
        let doc = test_doc(source);
        let result = ambient_analysis::analyze(source);
        let diagnostics = result.diagnostics();
        assert_eq!(diagnostics.len(), 1);

        let lsp = diagnostic_to_lsp(&doc, &diagnostics[0]);
        assert_eq!(lsp.severity, Some(DiagnosticSeverity::ERROR));
        assert!(lsp.range.start.line >= 1);
    }

    #[test]
    fn note_is_appended_to_message() {
        let doc = test_doc("fn f() { 1 }");
        let diagnostic = ambient_analysis::Diagnostic {
            span: ambient_engine::ast::Span::new(0, 2),
            message: "boom".to_string(),
            note: Some("extra context".to_string()),
            severity: ambient_analysis::Severity::Error,
        };
        let lsp = diagnostic_to_lsp(&doc, &diagnostic);
        assert!(lsp.message.contains("boom"));
        assert!(lsp.message.contains("note: extra context"));
    }
}

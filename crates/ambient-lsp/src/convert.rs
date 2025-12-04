//! Conversion utilities between Ambient types and LSP types.

use lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

use crate::documents::Document;

/// Convert an Ambient parse error to an LSP diagnostic.
pub fn parse_error_to_diagnostic(doc: &Document, error: &ambient_parser::ParseError) -> Diagnostic {
    let start = doc.offset_to_position(error.span.start as usize);
    let end = doc.offset_to_position(error.span.end as usize);

    let range = Range {
        start: Position {
            line: start.0,
            character: start.1,
        },
        end: Position {
            line: end.0,
            character: end.1,
        },
    };

    let message = error.kind.to_string();
    let related_info = error.context.as_ref().map(|ctx| format!("note: {ctx}"));

    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        code: None,
        code_description: None,
        source: Some("ambient".to_string()),
        message: if let Some(info) = related_info {
            format!("{message}\n{info}")
        } else {
            message
        },
        related_information: None,
        tags: None,
        data: None,
    }
}

/// Convert an Ambient type error to an LSP diagnostic.
pub fn type_error_to_diagnostic(
    doc: &Document,
    error: &ambient_engine::infer::TypeError,
) -> Diagnostic {
    let (start_offset, end_offset) = error.span;
    let start = doc.offset_to_position(start_offset as usize);
    let end = doc.offset_to_position(end_offset as usize);

    let range = Range {
        start: Position {
            line: start.0,
            character: start.1,
        },
        end: Position {
            line: end.0,
            character: end.1,
        },
    };

    let message = error.kind.to_string();
    let related_info = error.context.as_ref().map(|ctx| format!("note: {ctx}"));

    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        code: None,
        code_description: None,
        source: Some("ambient".to_string()),
        message: if let Some(info) = related_info {
            format!("{message}\n{info}")
        } else {
            message
        },
        related_information: None,
        tags: None,
        data: None,
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
    use ambient_engine::ast::Span;
    use ambient_engine::infer::{TypeError, TypeErrorKind};
    use ambient_engine::types::Type;
    use ambient_parser::{ParseError, ParseErrorKind};
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
    fn test_parse_error_to_diagnostic() {
        let doc = test_doc("fn foo( {");
        let error = ParseError::new(
            ParseErrorKind::Expected {
                expected: ")".into(),
                found: "{".into(),
            },
            Span::new(8, 9),
        );

        let diag = parse_error_to_diagnostic(&doc, &error);

        assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diag.source, Some("ambient".to_string()));
        assert_eq!(diag.range.start.line, 0);
        assert_eq!(diag.range.start.character, 8);
        assert!(diag.message.contains(")") || diag.message.contains("{"));
    }

    #[test]
    fn test_parse_error_unexpected_eof() {
        let doc = test_doc("fn foo(");
        let error = ParseError::new(ParseErrorKind::UnexpectedEof, Span::new(7, 7));

        let diag = parse_error_to_diagnostic(&doc, &error);

        assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
        assert!(diag.message.contains("EOF") || diag.message.contains("end"));
    }

    #[test]
    fn test_parse_error_with_context() {
        let doc = test_doc("fn foo() { let }");
        let mut error = ParseError::new(
            ParseErrorKind::Expected {
                expected: "identifier".into(),
                found: "}".into(),
            },
            Span::new(15, 16),
        );
        error.context = Some("in let binding".into());

        let diag = parse_error_to_diagnostic(&doc, &error);

        assert!(diag.message.contains("let binding"));
    }

    #[test]
    fn test_type_error_to_diagnostic() {
        let doc = test_doc("fn foo(): string { 42 }");
        let error = TypeError::new(
            TypeErrorKind::TypeMismatch {
                expected: Type::String,
                actual: Type::Number,
            },
            (19, 21),
        );

        let diag = type_error_to_diagnostic(&doc, &error);

        assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diag.source, Some("ambient".to_string()));
        assert_eq!(diag.range.start.line, 0);
        assert_eq!(diag.range.start.character, 19);
        assert!(diag.message.contains("mismatch") || diag.message.contains("type"));
    }

    #[test]
    fn test_type_error_with_context() {
        let doc = test_doc("fn foo() { 1 + true }");
        let mut error = TypeError::new(
            TypeErrorKind::TypeMismatch {
                expected: Type::Number,
                actual: Type::Bool,
            },
            (15, 19),
        );
        error.context = Some("in binary operation".into());

        let diag = type_error_to_diagnostic(&doc, &error);

        assert!(diag.message.contains("binary operation"));
    }

    #[test]
    fn test_parse_error_unterminated_string() {
        let doc = test_doc("let s = \"hello");
        let error = ParseError::new(ParseErrorKind::UnterminatedString, Span::new(8, 14));

        let diag = parse_error_to_diagnostic(&doc, &error);

        assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
        assert!(diag.message.contains("string") || diag.message.contains("unterminated"));
    }
}

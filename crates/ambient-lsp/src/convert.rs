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

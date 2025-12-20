//! Snapshot testing utilities for LSP responses.

use lsp_types::{
    CompletionItem, Diagnostic, DiagnosticSeverity, Hover, HoverContents, MarkedString,
};

/// Format hover information for snapshot testing.
pub fn format_hover_snapshot(hover: &Option<Hover>) -> String {
    match hover {
        Some(h) => {
            let content = match &h.contents {
                HoverContents::Scalar(scalar) => format_marked_string(scalar),
                HoverContents::Array(arr) => arr
                    .iter()
                    .map(format_marked_string)
                    .collect::<Vec<_>>()
                    .join("\n---\n"),
                HoverContents::Markup(markup) => markup.value.clone(),
            };

            let range_info = if let Some(range) = &h.range {
                format!(
                    "\n[range: {}:{}-{}:{}]",
                    range.start.line, range.start.character, range.end.line, range.end.character
                )
            } else {
                String::new()
            };

            format!("{content}{range_info}")
        }
        None => "(no hover)".to_string(),
    }
}

fn format_marked_string(ms: &MarkedString) -> String {
    match ms {
        MarkedString::String(s) => s.clone(),
        MarkedString::LanguageString(ls) => format!("```{}\n{}\n```", ls.language, ls.value),
    }
}

/// Format completions for snapshot testing.
///
/// The output is sorted alphabetically for stable comparisons.
pub fn format_completions_snapshot(items: &[CompletionItem]) -> String {
    let mut lines: Vec<String> = items
        .iter()
        .map(|item| {
            let kind = item
                .kind
                .map(|k| format!("{:?}", k))
                .unwrap_or_else(|| "Unknown".to_string());
            let detail = item.detail.as_deref().unwrap_or("");
            if detail.is_empty() {
                format!("{} ({})", item.label, kind)
            } else {
                format!("{} ({}) - {}", item.label, kind, detail)
            }
        })
        .collect();

    lines.sort();
    lines.join("\n")
}

/// Format diagnostics for snapshot testing.
pub fn format_diagnostics_snapshot(diagnostics: &[Diagnostic]) -> String {
    let mut lines: Vec<String> = diagnostics
        .iter()
        .map(|d| {
            let severity = match d.severity {
                Some(DiagnosticSeverity::ERROR) => "error",
                Some(DiagnosticSeverity::WARNING) => "warning",
                Some(DiagnosticSeverity::INFORMATION) => "info",
                Some(DiagnosticSeverity::HINT) => "hint",
                _ => "unknown",
            };

            format!(
                "[{}:{}] {}: {}",
                d.range.start.line + 1, // 1-indexed for readability
                d.range.start.character + 1,
                severity,
                d.message
            )
        })
        .collect();

    lines.sort();

    if lines.is_empty() {
        "(no diagnostics)".to_string()
    } else {
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range};

    #[test]
    fn test_format_hover_none() {
        assert_eq!(format_hover_snapshot(&None), "(no hover)");
    }

    #[test]
    fn test_format_hover_with_content() {
        let hover = Hover {
            contents: HoverContents::Scalar(MarkedString::String(
                "```ambient\nx: number\n```".to_string(),
            )),
            range: None,
        };
        let formatted = format_hover_snapshot(&Some(hover));
        assert!(formatted.contains("number"));
    }

    #[test]
    fn test_format_completions_empty() {
        assert_eq!(format_completions_snapshot(&[]), "");
    }

    #[test]
    fn test_format_diagnostics_empty() {
        assert_eq!(format_diagnostics_snapshot(&[]), "(no diagnostics)");
    }

    #[test]
    fn test_format_diagnostics_with_error() {
        let diags = vec![Diagnostic {
            range: Range {
                start: Position {
                    line: 0,
                    character: 5,
                },
                end: Position {
                    line: 0,
                    character: 10,
                },
            },
            severity: Some(DiagnosticSeverity::ERROR),
            message: "type mismatch".to_string(),
            ..Default::default()
        }];

        let formatted = format_diagnostics_snapshot(&diags);
        assert!(formatted.contains("[1:6]")); // 1-indexed
        assert!(formatted.contains("error"));
        assert!(formatted.contains("type mismatch"));
    }
}

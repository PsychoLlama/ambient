//! Syntax highlighting for the REPL.
//!
//! Classification is driven by the real language lexer via
//! [`ambient_parser::tokenize_for_highlighting`], so highlighting can never
//! drift from the grammar: a keyword or literal form added to the lexer is
//! highlighted here automatically, with no parallel token list to maintain.
//! Lexing is error-tolerant (the REPL re-highlights on every keystroke, so
//! most inputs are mid-edit); error regions render uncolored.

use std::borrow::Cow;

use ambient_parser::{TokenKind, tokenize_for_highlighting};
use rustyline::highlight::Highlighter;

/// ANSI color codes for syntax highlighting.
mod colors {
    pub const RESET: &str = "\x1b[0m";
    pub const KEYWORD: &str = "\x1b[1;35m"; // Bold magenta
    pub const STRING: &str = "\x1b[32m"; // Green
    pub const NUMBER: &str = "\x1b[33m"; // Yellow
    pub const COMMENT: &str = "\x1b[90m"; // Gray
    pub const OPERATOR: &str = "\x1b[36m"; // Cyan
    pub const BOOLEAN: &str = "\x1b[33m"; // Yellow (same as number)
    pub const PROMPT: &str = "\x1b[1;36m"; // Bold cyan
}

/// Syntax highlighter for the Ambient REPL.
#[derive(Default)]
pub struct AmbientHighlighter;

impl Highlighter for AmbientHighlighter {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        Cow::Owned(highlight_ambient(line))
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _forced: bool) -> bool {
        // Return true to re-highlight on every character
        true
    }

    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        _default: bool,
    ) -> Cow<'b, str> {
        Cow::Owned(format!("{}{prompt}{}", colors::PROMPT, colors::RESET))
    }
}

/// Map a token kind to its ANSI color, or `None` to leave it unstyled.
fn color_for(kind: TokenKind) -> Option<&'static str> {
    match kind {
        TokenKind::True | TokenKind::False => Some(colors::BOOLEAN),
        // Every other keyword, current and future, comes from the lexer's own
        // keyword table — never a string list that could drift.
        kind if kind.as_keyword_str().is_some() => Some(colors::KEYWORD),
        TokenKind::String
        | TokenKind::StringStart
        | TokenKind::StringMiddle
        | TokenKind::StringEnd => Some(colors::STRING),
        TokenKind::Number | TokenKind::Uuid => Some(colors::NUMBER),
        TokenKind::Comment | TokenKind::DocComment | TokenKind::InnerDocComment => {
            Some(colors::COMMENT)
        }
        TokenKind::Plus
        | TokenKind::Minus
        | TokenKind::Star
        | TokenKind::Slash
        | TokenKind::Percent
        | TokenKind::EqEq
        | TokenKind::Ne
        | TokenKind::Lt
        | TokenKind::Le
        | TokenKind::Gt
        | TokenKind::Ge
        | TokenKind::AndAnd
        | TokenKind::OrOr
        | TokenKind::Bang
        | TokenKind::Eq
        | TokenKind::FatArrow
        | TokenKind::Arrow => Some(colors::OPERATOR),
        // Identifiers, punctuation, whitespace, and error/unknown regions stay
        // uncolored.
        _ => None,
    }
}

/// Highlight Ambient source code with ANSI colors.
///
/// The output round-trips: stripping the ANSI escapes yields the input
/// exactly. Bytes the lexer's error recovery skips over are copied through
/// verbatim and uncolored.
fn highlight_ambient(input: &str) -> String {
    let mut result = String::with_capacity(input.len() * 2);
    let mut cursor = 0usize;

    for (span, kind) in tokenize_for_highlighting(input) {
        let start = usize::try_from(span.start).unwrap_or(usize::MAX);
        let end = usize::try_from(span.end).unwrap_or(usize::MAX);

        // Copy any bytes error recovery skipped over.
        if start > cursor {
            result.push_str(&input[cursor..start]);
        }

        let text = &input[start..end];
        if let Some(color) = color_for(kind) {
            result.push_str(color);
            result.push_str(text);
            result.push_str(colors::RESET);
        } else {
            result.push_str(text);
        }
        cursor = end;
    }

    // Copy any trailing bytes not covered by a token.
    result.push_str(&input[cursor..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Remove ANSI escape sequences (`ESC [ ... m`) from a string.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c2 in chars.by_ref() {
                    if c2 == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn assert_round_trips(input: &str) {
        let highlighted = highlight_ambient(input);
        assert_eq!(strip_ansi(&highlighted), input, "input: {input:?}");
    }

    #[test]
    fn test_round_trips_exactly() {
        for input in [
            "",
            "pub fn greet(name: String): String { \"hi ${name}\" }",
            "let x = 1 + 2.5e3 // trailing comment",
            "/// doc comment",
            "match x { Some(y) => y, None => 0 }",
            "struct Point unique(01234567-89AB-CDEF-0123-456789ABCDEF) { x: Number }",
            // Unterminated string, mid-keystroke.
            "let s = \"unfinished",
            "let s = \"half ${interp",
            // Invalid escape and stray characters.
            "\"bad \\q escape\"",
            "1 @ 2 # 3",
            "a & b | c",
            // Multi-byte UTF-8, inside and outside strings.
            "let greeting = \"héllo wörld 🦀\"",
            "café ← è",
            "\"unterminated é🦀",
        ] {
            assert_round_trips(input);
        }
    }

    #[test]
    fn test_keywords_colored_from_lexer_table() {
        // `struct` and `extern` were missing from the old hand-rolled keyword
        // list; the lexer-driven classifier covers every keyword.
        for keyword in TokenKind::all_keywords() {
            let highlighted = highlight_ambient(keyword);
            let color = if *keyword == "true" || *keyword == "false" {
                colors::BOOLEAN
            } else {
                colors::KEYWORD
            };
            assert_eq!(
                highlighted,
                format!("{color}{keyword}{}", colors::RESET),
                "keyword: {keyword}"
            );
        }
    }

    #[test]
    fn test_literal_and_comment_colors() {
        assert_eq!(
            highlight_ambient("42"),
            format!("{}42{}", colors::NUMBER, colors::RESET)
        );
        assert_eq!(
            highlight_ambient("\"hi\""),
            format!("{}\"hi\"{}", colors::STRING, colors::RESET)
        );
        assert_eq!(
            highlight_ambient("// note"),
            format!("{}// note{}", colors::COMMENT, colors::RESET)
        );
        assert_eq!(
            highlight_ambient("+"),
            format!("{}+{}", colors::OPERATOR, colors::RESET)
        );
    }

    #[test]
    fn test_identifiers_punctuation_and_errors_uncolored() {
        assert_eq!(highlight_ambient("foo_bar"), "foo_bar");
        assert_eq!(highlight_ambient("(,;)"), "(,;)");
        // Stray characters are lexer errors and stay uncolored.
        assert_eq!(highlight_ambient("@"), "@");
    }

    #[test]
    fn test_unterminated_string_still_reads_as_string() {
        let highlighted = highlight_ambient("\"unfinished");
        assert!(highlighted.starts_with(colors::STRING));
        assert_round_trips("\"unfinished");
    }

    #[test]
    fn test_prompt_is_colored() {
        let highlighter = AmbientHighlighter;
        let prompt = highlighter.highlight_prompt("ambient> ", true);
        assert_eq!(prompt, "\x1b[1;36mambient> \x1b[0m");
    }
}

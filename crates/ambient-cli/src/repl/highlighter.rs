//! Syntax highlighting for the REPL.

use std::borrow::Cow;

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
    pub const ABILITY: &str = "\x1b[1;34m"; // Bold blue
}

/// Keywords in the Ambient language.
const KEYWORDS: &[&str] = &[
    "fn", "pub", "let", "const", "if", "else", "match", "enum", "type", "ability", "use", "with",
    "handle", "resume", "sandbox", "unique",
];

/// Built-in type names and abilities.
const BUILTINS: &[&str] = &[
    "Console",
    "Filesystem",
    "Network",
    "Time",
    "Random",
    "Log",
    "Exception",
    "Async",
    "Option",
    "Result",
    "List",
    "Map",
    "Set",
    "Some",
    "None",
    "Ok",
    "Err",
];

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
        // Highlight the prompt in bold cyan
        Cow::Owned(format!("\x1b[1;36m{prompt}\x1b[0m"))
    }
}

/// Highlight Ambient source code with ANSI colors.
fn highlight_ambient(input: &str) -> String {
    let mut result = String::with_capacity(input.len() * 2);
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let c = chars[i];

        // Comments
        if c == '/' && i + 1 < len && chars[i + 1] == '/' {
            result.push_str(colors::COMMENT);
            while i < len {
                result.push(chars[i]);
                i += 1;
            }
            result.push_str(colors::RESET);
            continue;
        }

        // Strings
        if c == '"' {
            result.push_str(colors::STRING);
            result.push(c);
            i += 1;
            while i < len {
                let sc = chars[i];
                result.push(sc);
                i += 1;
                if sc == '"' {
                    break;
                }
                if sc == '\\' && i < len {
                    result.push(chars[i]);
                    i += 1;
                }
            }
            result.push_str(colors::RESET);
            continue;
        }

        // Numbers
        if c.is_ascii_digit() {
            result.push_str(colors::NUMBER);
            while i < len && (chars[i].is_ascii_digit() || chars[i] == '.') {
                result.push(chars[i]);
                i += 1;
            }
            result.push_str(colors::RESET);
            continue;
        }

        // Identifiers/keywords
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();

            if KEYWORDS.contains(&word.as_str()) {
                result.push_str(colors::KEYWORD);
                result.push_str(&word);
                result.push_str(colors::RESET);
            } else if word == "true" || word == "false" {
                result.push_str(colors::BOOLEAN);
                result.push_str(&word);
                result.push_str(colors::RESET);
            } else if BUILTINS.contains(&word.as_str()) {
                result.push_str(colors::ABILITY);
                result.push_str(&word);
                result.push_str(colors::RESET);
            } else {
                result.push_str(&word);
            }
            continue;
        }

        // Operators
        if "+-*/%=<>!&|".contains(c) {
            result.push_str(colors::OPERATOR);
            result.push(c);
            // Handle two-character operators
            if i + 1 < len {
                let next = chars[i + 1];
                let is_two_char = matches!(
                    (c, next),
                    ('=' | '!' | '<' | '>', '=')
                        | ('&', '&')
                        | ('|', '|')
                        | ('=', '>')
                        | ('-', '>')
                );
                if is_two_char {
                    result.push(next);
                    i += 1;
                }
            }
            result.push_str(colors::RESET);
            i += 1;
            continue;
        }

        // Other characters pass through
        result.push(c);
        i += 1;
    }

    result
}

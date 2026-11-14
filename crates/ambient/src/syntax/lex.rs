//! Source text as tokens. See `spec/syntax.md`.

use super::{Position, SyntaxError};

/// One token. Keywords are [`Token::Word`]s: which words are reserved
/// is the parser's to say, where it knows what it expects.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Token {
    Word(String),
    Int(i64),
    Float(f64),
    Text(String),
    /// The lines of a `///` comment, joined.
    Doc(String),
    /// Punctuation, as written.
    Mark(&'static str),
    End,
}

/// Every mark, longest first, so the first that fits is the one meant.
const MARKS: [&str; 33] = [
    "...", "=>", "==", "!=", "<=", ">=", "&&", "||", "++", "::", "(", ")", "{", "}", "[", "]", "<",
    ">", ",", ";", ":", ".", "=", "+", "-", "*", "/", "%", "!", "|", "&", "?", "@",
];

pub(super) struct Lexed {
    pub(super) tokens: Vec<Token>,
    /// Where each token starts.
    pub(super) positions: Vec<Position>,
}

struct Lexer<'a> {
    rest: &'a str,
    at: Position,
}

pub(super) fn lex(source: &str) -> Result<Lexed, SyntaxError> {
    let mut lexer = Lexer {
        rest: source,
        at: Position { line: 1, column: 1 },
    };
    let mut out = Lexed {
        tokens: Vec::new(),
        positions: Vec::new(),
    };
    loop {
        lexer.skip();
        let at = lexer.at;
        let token = lexer.token()?;
        // Consecutive doc lines are one comment.
        if let (Token::Doc(line), Some(Token::Doc(doc))) = (&token, out.tokens.last_mut()) {
            doc.push('\n');
            doc.push_str(line);
            continue;
        }
        let end = token == Token::End;
        out.tokens.push(token);
        out.positions.push(at);
        if end {
            return Ok(out);
        }
    }
}

impl Lexer<'_> {
    fn fail<T>(&self, message: &str) -> Result<T, SyntaxError> {
        Err(SyntaxError {
            message: message.to_owned(),
            at: self.at,
        })
    }

    /// Takes `count` bytes.
    fn take(&mut self, count: usize) -> &str {
        let (taken, rest) = self.rest.split_at(count);
        for c in taken.chars() {
            if c == '\n' {
                self.at.line += 1;
                self.at.column = 1;
            } else {
                self.at.column += 1;
            }
        }
        self.rest = rest;
        taken
    }

    /// Takes characters while `keep` holds.
    fn take_while(&mut self, keep: impl Fn(char) -> bool) -> &str {
        let end = self.rest.find(|c| !keep(c)).unwrap_or(self.rest.len());
        self.take(end)
    }

    /// Skips whitespace and comments that are not doc comments.
    fn skip(&mut self) {
        loop {
            self.take_while(char::is_whitespace);
            let comment = self.rest.starts_with("//")
                && (!self.rest.starts_with("///") || self.rest.starts_with("////"));
            if !comment {
                return;
            }
            self.take_while(|c| c != '\n');
        }
    }

    fn token(&mut self) -> Result<Token, SyntaxError> {
        let Some(first) = self.rest.chars().next() else {
            return Ok(Token::End);
        };
        if self.rest.starts_with("///") {
            self.take(3);
            let line = self.take_while(|c| c != '\n');
            return Ok(Token::Doc(
                line.strip_prefix(' ').unwrap_or(line).to_owned(),
            ));
        }
        if first.is_alphabetic() || first == '_' {
            let word = self.take_while(|c| c.is_alphanumeric() || c == '_');
            return Ok(Token::Word(word.to_owned()));
        }
        if first.is_ascii_digit() {
            return self.number();
        }
        if first == '"' {
            return self.text();
        }
        match MARKS.iter().find(|mark| self.rest.starts_with(**mark)) {
            Some(mark) => {
                self.take(mark.len());
                Ok(Token::Mark(mark))
            }
            None => self.fail(&format!("`{first}` is not part of any token")),
        }
    }

    fn number(&mut self) -> Result<Token, SyntaxError> {
        let digit = |c: char| c.is_ascii_digit() || c == '_';
        let mut written = self.take_while(digit).to_owned();
        // A dot is a fraction only before a digit: `1.to_text()` is a call.
        let mut after = self.rest.chars().skip(1);
        let fraction =
            self.rest.starts_with('.') && after.next().is_some_and(|c| c.is_ascii_digit());
        if fraction {
            written.push_str(self.take(1));
            written.push_str(self.take_while(digit));
        }
        written.retain(|c| c != '_');
        let token = if fraction {
            written.parse().ok().map(Token::Float)
        } else {
            written.parse().ok().map(Token::Int)
        };
        match token {
            Some(token) => Ok(token),
            None => self.fail("a number out of range"),
        }
    }

    fn text(&mut self) -> Result<Token, SyntaxError> {
        self.take(1);
        let mut text = String::new();
        loop {
            text.push_str(self.take_while(|c| !matches!(c, '"' | '\\' | '\n')));
            match self.rest.chars().next() {
                Some('"') => {
                    self.take(1);
                    return Ok(Token::Text(text));
                }
                Some('\\') => {
                    self.take(1);
                    text.push(self.escape()?);
                }
                _ => return self.fail("a string that does not end on its line"),
            }
        }
    }

    /// The character an escape stands for, after its backslash.
    fn escape(&mut self) -> Result<char, SyntaxError> {
        let Some(escaped) = self.rest.chars().next() else {
            return self.fail("a string that does not end");
        };
        let stands_for = match escaped {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            '0' => '\0',
            '"' | '\\' => escaped,
            'u' => return self.scalar(),
            other => return self.fail(&format!("`\\{other}` is not an escape")),
        };
        self.take(escaped.len_utf8());
        Ok(stands_for)
    }

    /// `u{hex}`, a character by its number.
    fn scalar(&mut self) -> Result<char, SyntaxError> {
        self.take(1);
        if !self.rest.starts_with('{') {
            return self.fail("a `\\u` escape is written `\\u{hex}`");
        }
        self.take(1);
        let hex = self.take_while(|c| c.is_ascii_hexdigit()).to_owned();
        let scalar = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32);
        match (scalar, self.rest.starts_with('}')) {
            (Some(scalar), true) => {
                self.take(1);
                Ok(scalar)
            }
            _ => self.fail("a `\\u` escape is written `\\u{hex}`, and names a character"),
        }
    }
}

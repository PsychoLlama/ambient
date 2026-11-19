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
    /// Bytes written out: `0x[89 50]`.
    Bytes(Vec<u8>),
    /// The lines of a `///` comment, joined.
    Doc(String),
    /// Punctuation, as written.
    Mark(&'static str),
    End,
}

/// Every mark, longest first, so the first that fits is the one meant.
const MARKS: [&str; 33] = [
    "...", "=>", "==", "!=", "<=", ">=", "&&", "||", "++", "::", "(", ")", "{", "}", "[", "]", "<",
    ">", ",", ";", ":", ".", "=", "+", "-", "*", "/", "%", "!", "|", "&", "?", "#",
];

pub(super) struct Lexed {
    pub(super) tokens: Vec<Token>,
    /// Where each token starts.
    pub(super) positions: Vec<Position>,
}

struct Lexer<'a> {
    rest: &'a str,
    at: Position,
    out: Lexed,
}

pub(super) fn lex(source: &str) -> Result<Lexed, SyntaxError> {
    let mut lexer = Lexer {
        rest: source,
        at: Position { line: 1, column: 1 },
        out: Lexed {
            tokens: Vec::new(),
            positions: Vec::new(),
        },
    };
    while lexer.next()? != Some(Token::End) {}
    Ok(lexer.out)
}

impl Lexer<'_> {
    /// Reads the next token into what is read so far, and gives it. A
    /// string with values written into it is several, and gives none.
    fn next(&mut self) -> Result<Option<Token>, SyntaxError> {
        self.skip();
        let at = self.at;
        if self.rest.starts_with('"') {
            return self.text();
        }
        let token = self.token()?;
        // Consecutive doc lines are one comment.
        if let (Token::Doc(line), Some(Token::Doc(doc))) = (&token, self.out.tokens.last_mut()) {
            doc.push('\n');
            doc.push_str(line);
        } else {
            self.push(token.clone(), at);
        }
        Ok(Some(token))
    }

    fn push(&mut self, token: Token, at: Position) {
        self.out.tokens.push(token);
        self.out.positions.push(at);
    }

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
        if self.rest.starts_with("0x[") {
            return self.bytes();
        }
        if first.is_ascii_digit() {
            return self.number();
        }
        match MARKS.iter().find(|mark| self.rest.starts_with(**mark)) {
            Some(mark) => {
                self.take(mark.len());
                Ok(Token::Mark(mark))
            }
            None => self.fail(&format!("`{first}` is not part of any token")),
        }
    }

    /// Bytes written out: pairs of hexadecimal digits, a byte each,
    /// with any space between pairs.
    fn bytes(&mut self) -> Result<Token, SyntaxError> {
        self.take(3);
        let mut bytes = Vec::new();
        loop {
            self.take_while(char::is_whitespace);
            if self.rest.starts_with(']') {
                self.take(1);
                return Ok(Token::Bytes(bytes));
            }
            let pair = self.rest.get(..2).unwrap_or_default();
            let digits = pair.len() == 2 && pair.chars().all(|c| c.is_ascii_hexdigit());
            match u8::from_str_radix(pair, 16) {
                Ok(byte) if digits => bytes.push(byte),
                _ => return self.fail("a byte is written as two hexadecimal digits"),
            }
            self.take(2);
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
        // An exponent is one only before a digit, signed or not: `1e3`.
        let mut after = self.rest.chars().skip(1);
        let signed = after.clone().next().is_some_and(|c| c == '+' || c == '-');
        let power = after.nth(usize::from(signed));
        let exponent =
            self.rest.starts_with(['e', 'E']) && power.is_some_and(|c| c.is_ascii_digit());
        if exponent {
            written.push_str(self.take(1 + usize::from(signed)));
            written.push_str(self.take_while(digit));
        }
        written.retain(|c| c != '_');
        let token = if fraction || exponent {
            written.parse().ok().map(Token::Float)
        } else {
            written.parse().ok().map(Token::Int)
        };
        match token {
            Some(token) => Ok(token),
            None => self.fail("a number out of range"),
        }
    }

    /// A string. One with values written into it (`"at ${place}"`) is
    /// read as what it means, in tokens: its pieces joined, each value
    /// asked for its text, and the whole in brackets.
    fn text(&mut self) -> Result<Option<Token>, SyntaxError> {
        let start = self.at;
        let before = self.out.tokens.len();
        self.take(1);
        let mut text = String::new();
        loop {
            text.push_str(self.take_while(|c| !matches!(c, '"' | '\\' | '\n' | '$')));
            match self.rest.chars().next() {
                Some('"') => {
                    self.take(1);
                    break;
                }
                Some('\\') => {
                    self.take(1);
                    text.push(self.escape()?);
                }
                Some('$') if self.rest.starts_with("${") => {
                    let piece = std::mem::take(&mut text);
                    self.piece(before, start, Some(piece).filter(|piece| !piece.is_empty()));
                    self.value(before)?;
                }
                Some('$') => text.push_str(self.take(1)),
                _ => return self.fail("a string that does not end on its line"),
            }
        }
        if self.out.tokens.len() == before {
            self.push(Token::Text(text.clone()), start);
            return Ok(Some(Token::Text(text)));
        }
        self.piece(before, start, Some(text).filter(|text| !text.is_empty()));
        self.push(Token::Mark(")"), start);
        Ok(None)
    }

    /// Adds a piece of a string whose tokens began at `before`: text,
    /// or nothing but the join before a value. The first piece opens
    /// the bracket the whole string is in.
    fn piece(&mut self, before: usize, start: Position, text: Option<String>) {
        if self.out.tokens.len() == before {
            self.push(Token::Mark("("), start);
        } else if text.is_some() {
            self.push(Token::Mark("++"), start);
        }
        if let Some(text) = text {
            self.push(Token::Text(text), start);
        }
    }

    /// `${value}` in a string whose tokens began at `before`: the tokens
    /// of the value, in brackets, asked for its text.
    fn value(&mut self, before: usize) -> Result<(), SyntaxError> {
        let at = self.at;
        self.take(2);
        // Something is already there, if only the opening bracket.
        if self.out.tokens.len() > before + 1 {
            self.push(Token::Mark("++"), at);
        }
        self.push(Token::Mark("("), at);
        let first = self.out.tokens.len();
        let mut depth = 0;
        loop {
            self.skip();
            if depth == 0 && self.rest.starts_with('}') {
                break;
            }
            match self.next()? {
                Some(Token::Mark("{")) => depth += 1,
                Some(Token::Mark("}")) => depth -= 1,
                Some(Token::End) => {
                    return Err(SyntaxError {
                        message: "a `${` in a string is not closed".to_owned(),
                        at,
                    });
                }
                _ => {}
            }
        }
        if self.out.tokens.len() == first {
            return self.fail("nothing is written between `${` and `}`");
        }
        self.take(1);
        let asked = [
            Token::Mark(")"),
            Token::Mark("."),
            Token::Word("to_text".to_owned()),
            Token::Mark("("),
            Token::Mark(")"),
        ];
        for token in asked {
            self.push(token, at);
        }
        Ok(())
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
            '"' | '\\' | '$' => escaped,
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

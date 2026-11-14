//! The reader's place in the tokens, and what every part of it asks.

mod block;
mod definitions;
mod expr;
mod patterns;
mod record;
mod scopes;
mod types;

use super::lex::{Lexed, Token};
use super::{Position, SyntaxError};
use crate::surface::{Path, Type};

pub(super) type Parsed<T> = Result<T, SyntaxError>;

/// Words that never name anything.
const RESERVED: [&str; 34] = [
    "fn", "let", "return", "if", "else", "match", "type", "ability", "key", "tag", "extern",
    "target", "import", "pub", "with", "as", "try", "budget", "provide", "only", "observe",
    "group", "spawn", "all", "first", "cell", "comptime", "using", "true", "false", "null",
    "never", "unknown", "module",
];

pub(super) struct Parser {
    tokens: Vec<Token>,
    positions: Vec<Position>,
    index: usize,
    /// The locals in scope where the reader is, innermost last.
    locals: Vec<String>,
    /// The type a method that states no need needs: the definition being
    /// read, while its own record is.
    own: Option<Type>,
    /// How many names the reader has made up.
    made: usize,
}

impl Parser {
    pub(super) fn new(lexed: Lexed) -> Self {
        Self {
            tokens: lexed.tokens,
            positions: lexed.positions,
            index: 0,
            locals: Vec::new(),
            own: None,
            made: 0,
        }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.index]
    }

    /// The token `ahead` places after the next.
    fn peek_at(&self, ahead: usize) -> &Token {
        let last = self.tokens.len() - 1;
        &self.tokens[(self.index + ahead).min(last)]
    }

    fn position(&self) -> Position {
        self.positions[self.index]
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.index].clone();
        if self.index + 1 < self.tokens.len() {
            self.index += 1;
        }
        token
    }

    fn fail<T>(&self, message: impl Into<String>) -> Parsed<T> {
        self.fail_at(self.position(), message)
    }

    #[expect(clippy::unused_self, reason = "reads as the other failures do")]
    fn fail_at<T>(&self, at: Position, message: impl Into<String>) -> Parsed<T> {
        Err(SyntaxError {
            message: message.into(),
            at,
        })
    }

    /// Fails for finding something other than `wanted`.
    fn unexpected<T>(&self, wanted: &str) -> Parsed<T> {
        let found = match self.peek() {
            Token::Word(word) => format!("`{word}`"),
            Token::Int(value) => format!("`{value}`"),
            Token::Float(value) => format!("`{value}`"),
            Token::Text(_) => "a string".to_owned(),
            Token::Doc(_) => "a doc comment".to_owned(),
            Token::Mark(mark) => format!("`{mark}`"),
            Token::End => "the end of the file".to_owned(),
        };
        self.fail(format!("expected {wanted}, found {found}"))
    }

    fn at(&self, mark: &str) -> bool {
        matches!(self.peek(), Token::Mark(found) if *found == mark)
    }

    fn at_word(&self, word: &str) -> bool {
        matches!(self.peek(), Token::Word(found) if found == word)
    }

    fn eat(&mut self, mark: &str) -> bool {
        let found = self.at(mark);
        if found {
            self.advance();
        }
        found
    }

    fn eat_word(&mut self, word: &str) -> bool {
        let found = self.at_word(word);
        if found {
            self.advance();
        }
        found
    }

    fn expect(&mut self, mark: &str) -> Parsed<()> {
        if self.eat(mark) {
            Ok(())
        } else {
            self.unexpected(&format!("`{mark}`"))
        }
    }

    fn expect_word(&mut self, word: &str) -> Parsed<()> {
        if self.eat_word(word) {
            Ok(())
        } else {
            self.unexpected(&format!("`{word}`"))
        }
    }

    /// Whether the next token is a word that may name something.
    fn at_name(&self) -> bool {
        matches!(self.peek(), Token::Word(word) if !RESERVED.contains(&word.as_str()))
    }

    /// A name: a word that is not reserved, and not one the reader could
    /// make up.
    fn name(&mut self) -> Parsed<String> {
        match self.peek() {
            Token::Word(word) if word.starts_with("__") => {
                self.fail("a name does not start with two underscores")
            }
            Token::Word(word) if !RESERVED.contains(&word.as_str()) => {
                let word = word.clone();
                self.advance();
                Ok(word)
            }
            _ => self.unexpected("a name"),
        }
    }

    /// A name no program can write.
    fn made_up(&mut self, what: &str) -> String {
        self.made += 1;
        format!("__{what}{}", self.made)
    }

    /// Names separated by dots.
    fn path(&mut self) -> Parsed<Path> {
        let mut segments = vec![self.name()?];
        while self.at(".") && matches!(self.peek_at(1), Token::Word(_)) {
            self.advance();
            segments.push(self.name()?);
        }
        Ok(Path(segments))
    }

    /// `item`s separated by commas, up to `close`, which is taken. A
    /// comma may follow the last.
    fn list<T>(
        &mut self,
        close: &str,
        mut item: impl FnMut(&mut Self) -> Parsed<T>,
    ) -> Parsed<Vec<T>> {
        let mut items = Vec::new();
        while !self.eat(close) {
            items.push(item(self)?);
            if !self.eat(",") {
                self.expect(close)?;
                break;
            }
        }
        Ok(items)
    }

    /// Reads with the locals `read` adds in scope, and takes them out of
    /// scope after.
    fn scoped<T>(&mut self, read: impl FnOnce(&mut Self) -> Parsed<T>) -> Parsed<T> {
        let outside = self.locals.len();
        let result = read(self);
        self.locals.truncate(outside);
        result
    }

    fn is_local(&self, name: &str) -> bool {
        self.locals.iter().any(|local| local == name)
    }
}

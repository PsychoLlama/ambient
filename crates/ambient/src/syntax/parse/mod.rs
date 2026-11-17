//! The reader's place in the tokens, and what every part of it asks.

mod block;
mod definitions;
mod expr;
mod patterns;
mod record;
mod scopes;
mod types;

use super::lex::{Lexed, Token};
use super::{Placed, Position, SyntaxError};
use crate::surface::{Expr, Path, Type};

pub(super) type Parsed<T> = Result<T, SyntaxError>;

/// Words that never name anything. A word that only begins a definition
/// (`key`, `tag`, `ability`, `target`, `extern`, `import`, `module`) is
/// not one: nothing else begins where a definition does.
const RESERVED: [&str; 25] = [
    "fn", "let", "return", "if", "else", "for", "match", "type", "pub", "with", "as", "try",
    "provide", "only", "observe", "group", "spawn", "cell", "comptime", "using", "true", "false",
    "null", "never", "unknown",
];

/// A function in a block, while its body is read. A use of its name
/// there is a read of the record it will be a method of.
pub(super) struct Own {
    /// Which of the locals it is.
    pub(super) local: usize,
    /// The name its body has for that record.
    pub(super) receiver: String,
    /// Whether its body named it.
    pub(super) used: bool,
}

pub(super) struct Parser {
    tokens: Vec<Token>,
    positions: Vec<Position>,
    index: usize,
    /// The locals in scope where the reader is, innermost last.
    locals: Vec<String>,
    /// The type a method that states no need needs: the definition being
    /// read, while its own record is.
    own: Option<Type>,
    /// The functions in a block whose bodies are being read, innermost
    /// last.
    calling: Vec<Own>,
    /// How many names the reader has made up.
    made: usize,
    /// Where each definition read so far is.
    defined: Vec<Placed>,
}

impl Parser {
    pub(super) fn new(lexed: Lexed) -> Self {
        Self {
            tokens: lexed.tokens,
            positions: lexed.positions,
            index: 0,
            locals: Vec::new(),
            own: None,
            calling: Vec::new(),
            made: 0,
            defined: Vec::new(),
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

    /// `expr`, marked as written at the token numbered `at`.
    #[expect(clippy::unused_self, reason = "reads as the other makers do")]
    fn mark(&self, at: usize, expr: Expr) -> Expr {
        Expr::At(at, Box::new(expr))
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

    /// What was written between parentheses that opened at `at`: the one
    /// thing they group, or the members of a tuple.
    fn grouped<T>(&self, at: Position, mut members: Vec<T>) -> Parsed<Result<T, Vec<T>>> {
        let trailing = matches!(self.peek_back(), Token::Mark(","));
        match members.len() {
            1 if !trailing => Ok(Ok(members.remove(0))),
            0 | 1 => self.fail_at(at, crate::builder::BuildError::ShortTuple.to_string()),
            _ => Ok(Err(members)),
        }
    }

    /// Fails for finding something other than `wanted`.
    fn unexpected<T>(&self, wanted: &str) -> Parsed<T> {
        let found = match self.peek() {
            Token::Word(word) => format!("`{word}`"),
            Token::Int(value) => format!("`{value}`"),
            Token::Float(value) => format!("`{value}`"),
            Token::Text(_) => "a string".to_owned(),
            Token::Bytes(_) => "bytes".to_owned(),
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

    /// The record a function in a block is a method of, if `name` is
    /// one whose body this is: nothing nearer has the name. The
    /// function is noted as naming itself.
    fn own_receiver(&mut self, name: &str) -> Option<String> {
        let local = self.locals.iter().rposition(|local| local == name)?;
        let own = self.calling.iter_mut().find(|own| own.local == local)?;
        own.used = true;
        Some(own.receiver.clone())
    }
}

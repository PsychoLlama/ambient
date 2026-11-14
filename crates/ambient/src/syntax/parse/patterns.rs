//! Patterns: what a match arm tests and what a `let` takes apart.

use super::super::lex::Token;
use super::{Parsed, Parser};
use crate::literal::Literal;
use crate::surface::Pattern;

/// Where a pattern is written.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    /// In a match arm: a bare name is a type or a tag, and `let` binds.
    Testing,
    /// After `let`: nothing is tested, so a bare name binds.
    Binding,
}

impl Parser {
    /// A pattern. The names it binds are added to `binds`, and are not
    /// in scope until whoever asked puts them there.
    pub(super) fn pattern(&mut self, mode: Mode, binds: &mut Vec<String>) -> Parsed<Pattern> {
        match self.peek().clone() {
            Token::Word(word) if word == "_" => {
                self.advance();
                Ok(Pattern::Wildcard)
            }
            Token::Word(word) if word == "let" && mode == Mode::Testing => {
                self.advance();
                let name = self.name()?;
                binds.push(name.clone());
                let inner = if self.eat(":") {
                    self.pattern(mode, binds)?
                } else {
                    Pattern::Wildcard
                };
                Ok(Pattern::Bind(name, Box::new(inner)))
            }
            Token::Mark("(") => {
                let at = self.position();
                self.advance();
                let members = self.list(")", |parser| parser.pattern(mode, binds))?;
                Ok(self.grouped(at, members)?.unwrap_or_else(Pattern::Tuple))
            }
            Token::Mark("{") => {
                self.advance();
                let fields = self.list("}", |parser| parser.field_pattern(mode, binds))?;
                Ok(Pattern::Record(fields))
            }
            Token::Int(value) => Ok(self.literal(Literal::Int(value))),
            Token::Float(value) => Ok(self.literal(Literal::float(value))),
            Token::Text(text) => Ok(self.literal(Literal::String(text))),
            Token::Word(word) if word == "true" => Ok(self.literal(Literal::Bool(true))),
            Token::Word(word) if word == "false" => Ok(self.literal(Literal::Bool(false))),
            Token::Mark("-") => {
                self.advance();
                match self.peek().clone() {
                    Token::Int(value) => Ok(self.literal(Literal::Int(-value))),
                    Token::Float(value) => Ok(self.literal(Literal::float(-value))),
                    _ => self.unexpected("a number"),
                }
            }
            // A name binds, and may say what it binds: `items: List<int>`
            // fits only a list, which is for a `let` with an `else`.
            Token::Word(word) if mode == Mode::Binding && word != "null" => {
                let name = self.name()?;
                binds.push(name.clone());
                let inner = if self.eat(":") {
                    Pattern::Is(self.ty()?)
                } else {
                    Pattern::Wildcard
                };
                Ok(Pattern::Bind(name, Box::new(inner)))
            }
            _ => Ok(Pattern::Is(self.ty()?)),
        }
    }

    fn literal(&mut self, literal: Literal) -> Pattern {
        self.advance();
        Pattern::Literal(literal)
    }

    /// A field of a record pattern: a name and a pattern for what it
    /// holds, or a binder alone, which binds the field of its name.
    fn field_pattern(&mut self, mode: Mode, binds: &mut Vec<String>) -> Parsed<(String, Pattern)> {
        let shorthand = match mode {
            Mode::Testing => self.eat_word("let"),
            Mode::Binding => !matches!(self.peek_at(1), Token::Mark(":")),
        };
        let name = self.name()?;
        if shorthand {
            binds.push(name.clone());
            return Ok((
                name.clone(),
                Pattern::Bind(name, Box::new(Pattern::Wildcard)),
            ));
        }
        self.expect(":")?;
        Ok((name, self.pattern(mode, binds)?))
    }
}

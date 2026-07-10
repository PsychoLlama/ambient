//! Recursive descent parser for the Ambient language.
//!
//! The parser consumes tokens from the lexer and builds a CST.
//! It implements error recovery to continue parsing after errors.
//!
//! # Module Organization
//!
//! The parser is organized into logical sections:
//!
//! - **Module parsing** - Top-level module and item parsing
//! - **Function parsing** - Function definitions and parameters
//! - **Other definitions** - Constants, type aliases, enums, abilities
//! - **Type parsing** - Type expressions and annotations (in `types.rs`)
//! - **Expression parsing** - All expression forms (in `expr.rs`)
//! - **Pattern parsing** - Pattern matching syntax (in `patterns.rs`)
//! - **Helpers** - Token consumption and error recovery utilities

// Allow expect() for internal invariants (e.g., segments.last() on non-empty vectors)
#![allow(clippy::expect_used)]

mod control;
mod expr;
mod items;
mod patterns;
#[cfg(test)]
mod tests;
mod types;
mod use_tree;

use ambient_engine::ast::Span;

use crate::cst::{
    CstIdent, CstModule, CstQualifiedName, CstReplInput, Trivia, TriviaItem, TriviaKind,
};
use crate::error::{ParseError, ParseErrorKind};
use crate::lexer::{Lexer, Token, TokenKind};

/// The parser for Ambient source code.
pub struct Parser<'src> {
    tokens: Vec<Token>,
    pos: usize,
    /// Collected errors for error recovery.
    errors: Vec<ParseError>,
    /// Phantom data to tie the parser's lifetime to the source.
    _marker: std::marker::PhantomData<&'src str>,
}

impl<'src> Parser<'src> {
    /// Create a new parser for the given source.
    ///
    /// # Errors
    ///
    /// Returns a `ParseError` if the source contains invalid tokens (e.g., unterminated
    /// strings, invalid escape sequences).
    pub fn new(source: &'src str) -> Result<Self, ParseError> {
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize()?;
        Ok(Self {
            tokens,
            pos: 0,
            errors: Vec::new(),
            _marker: std::marker::PhantomData,
        })
    }

    /// Get the current token.
    fn current(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or_else(|| {
            self.tokens
                .last()
                .expect("tokens should always contain at least EOF")
        })
    }

    /// Get the current token kind.
    fn current_kind(&self) -> TokenKind {
        self.current().kind
    }

    /// Check if we're at the end of file.
    fn at_end(&self) -> bool {
        self.current_kind() == TokenKind::Eof
    }

    /// Advance past any trivia, returning it.
    fn skip_trivia(&mut self) -> Trivia {
        let mut items = Vec::new();
        while self.current_kind().is_trivia() {
            let token = self.current().clone();
            let kind = match token.kind {
                TokenKind::Whitespace => TriviaKind::Whitespace,
                TokenKind::Comment => TriviaKind::Comment,
                TokenKind::DocComment => TriviaKind::DocComment,
                TokenKind::InnerDocComment => TriviaKind::InnerDocComment,
                _ => unreachable!(),
            };
            items.push(TriviaItem {
                kind,
                span: token.span,
                text: token.text.into(),
            });
            self.pos += 1;
        }
        Trivia { items }
    }

    /// Skip only module-level trivia (whitespace and inner doc comments).
    /// Leaves outer doc comments (`///`) for items to consume.
    fn skip_module_trivia(&mut self) -> Trivia {
        let mut items = Vec::new();
        while matches!(
            self.current_kind(),
            TokenKind::Whitespace | TokenKind::Comment | TokenKind::InnerDocComment
        ) {
            let token = self.current().clone();
            let kind = match token.kind {
                TokenKind::Whitespace => TriviaKind::Whitespace,
                TokenKind::Comment => TriviaKind::Comment,
                TokenKind::InnerDocComment => TriviaKind::InnerDocComment,
                _ => unreachable!(),
            };
            items.push(TriviaItem {
                kind,
                span: token.span,
                text: token.text.into(),
            });
            self.pos += 1;
        }
        Trivia { items }
    }

    /// Advance to the next token, skipping trivia.
    fn advance(&mut self) -> Token {
        self.skip_trivia();
        let token = self.current().clone();
        if !self.at_end() {
            self.pos += 1;
        }
        token
    }

    /// Check if current token matches the kind.
    fn check(&mut self, kind: TokenKind) -> bool {
        self.skip_trivia();
        self.current_kind() == kind
    }

    /// Consume a token if it matches, otherwise return None.
    fn consume(&mut self, kind: TokenKind) -> Option<Token> {
        if self.check(kind) {
            Some(self.advance())
        } else {
            None
        }
    }

    /// Expect a specific token, returning an error if not found.
    fn expect(&mut self, kind: TokenKind) -> Result<Token, ParseError> {
        if self.check(kind) {
            Ok(self.advance())
        } else {
            let found = self.current().clone();
            Err(ParseError::new(
                ParseErrorKind::Expected {
                    expected: format!("{kind:?}"),
                    found: format!("{:?}", found.kind),
                },
                found.span,
            ))
        }
    }

    /// Record an error and continue parsing.
    fn error(&mut self, error: ParseError) {
        self.errors.push(error);
    }

    /// Synchronize after an error by skipping to a recovery point.
    fn synchronize(&mut self) {
        loop {
            // Trivia must be skipped before matching: `advance()` in the
            // fallback arm consumes the first non-trivia token, so matching
            // on raw trivia here would swallow recovery points that follow
            // whitespace.
            self.skip_trivia();
            match self.current_kind() {
                TokenKind::Eof
                | TokenKind::Fn
                | TokenKind::Pub
                | TokenKind::Const
                | TokenKind::Type
                | TokenKind::Struct
                | TokenKind::Enum
                | TokenKind::Ability
                | TokenKind::Use
                | TokenKind::Extern
                | TokenKind::Trait
                | TokenKind::Impl => return,
                TokenKind::Semi | TokenKind::RBrace => {
                    self.advance();
                    return;
                }
                _ => {
                    self.advance();
                }
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Module parsing
    // ─────────────────────────────────────────────────────────────────────────

    /// Parse a complete module.
    ///
    /// # Errors
    ///
    /// Returns a `ParseError` if the source contains syntax errors.
    pub fn parse_module(&mut self) -> Result<CstModule, ParseError> {
        let (module, mut errors) = self.parse_module_recovering();
        if errors.is_empty() {
            Ok(module)
        } else {
            Err(errors.remove(0))
        }
    }

    /// Parse a complete module, recovering from item-level errors.
    ///
    /// An item that fails to parse is skipped — the parser resynchronizes at
    /// the next item boundary — and its error collected. The returned CST
    /// contains every item that parsed cleanly, so callers (IDE tooling in
    /// particular) can analyze the rest of the file alongside the errors.
    pub fn parse_module_recovering(&mut self) -> (CstModule, Vec<ParseError>) {
        let start = self.current().span.start;
        // Only consume module-level trivia (whitespace, regular comments, inner doc comments).
        // Leave outer doc comments (`///`) for items to consume.
        let leading_trivia = self.skip_module_trivia();

        let mut items = Vec::new();
        // Skip module-level trivia before checking at_end to handle trailing whitespace/newlines
        while {
            self.skip_module_trivia();
            !self.at_end()
        } {
            match self.parse_item() {
                Ok(item) => items.push(item),
                Err(e) => {
                    self.error(e);
                    self.synchronize();
                }
            }
        }

        let trailing_trivia = self.skip_trivia();
        let end = self.current().span.end;

        let module = CstModule {
            name: "".into(), // Set by caller based on file path
            leading_trivia,
            items,
            trailing_trivia,
            span: Span::new(start, end),
        };
        (module, std::mem::take(&mut self.errors))
    }

    /// Parse REPL input, which may be either an item (function, const, etc.) or an expression.
    ///
    /// This is used by the REPL to support defining functions and constants interactively.
    ///
    /// # Errors
    ///
    /// Returns a `ParseError` if the source is not a valid item or expression.
    pub fn parse_repl_input(&mut self) -> Result<CstReplInput, ParseError> {
        self.skip_trivia();

        // Check if this looks like an item definition
        let is_item = matches!(
            self.current_kind(),
            TokenKind::Fn
                | TokenKind::Pub
                | TokenKind::Const
                | TokenKind::Type
                | TokenKind::Struct
                | TokenKind::Unique
                | TokenKind::Enum
                | TokenKind::Ability
                | TokenKind::Use
                | TokenKind::Trait
                | TokenKind::Impl
        );

        if is_item {
            let item = self.parse_item()?;
            self.skip_trivia();
            if !self.at_end() {
                return Err(ParseError::new(
                    ParseErrorKind::UnexpectedToken(format!("{:?}", self.current_kind())),
                    self.current().span,
                ));
            }
            Ok(CstReplInput::Item(Box::new(item)))
        } else {
            let expr = self.parse_expression()?;
            self.skip_trivia();
            if !self.at_end() {
                return Err(ParseError::new(
                    ParseErrorKind::UnexpectedToken(format!("{:?}", self.current_kind())),
                    self.current().span,
                ));
            }
            Ok(CstReplInput::Expr(expr))
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Helpers
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_ident(&mut self) -> Result<CstIdent, ParseError> {
        let token = self.expect(TokenKind::Ident)?;
        let trailing_trivia = self.skip_trivia();
        Ok(CstIdent {
            name: token.text.into(),
            span: token.span,
            trailing_trivia,
        })
    }

    fn parse_qualified_name(&mut self) -> Result<CstQualifiedName, ParseError> {
        let mut segments = Vec::new();
        let start = self.current().span.start;

        // A path may be rooted at a keyword (`pkg::m::T`, `core::collections::list`,
        // `self::m::T`, `super::m::T`); segments after the head are plain
        // identifiers. `parse_use_segment` accepts exactly this set.
        segments.push(self.parse_use_segment()?);

        while self.consume(TokenKind::ColonColon).is_some() {
            if !self.check(TokenKind::Ident) && !self.check(TokenKind::Super) {
                break;
            }
            segments.push(self.parse_use_segment()?);
        }

        let end = segments.last().expect("segments not empty").span.end;
        Ok(CstQualifiedName {
            segments,
            span: Span::new(start, end),
        })
    }

    pub(crate) fn unescape_string(text: &str) -> String {
        // Remove the opening and closing quotes (exactly one from each end)
        let content = text
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(text);
        Self::unescape_string_part(content)
    }

    pub(crate) fn unescape_string_part(text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => result.push('\n'),
                    Some('r') => result.push('\r'),
                    Some('t') => result.push('\t'),
                    Some('"') => result.push('"'),
                    Some('$') => result.push('$'),
                    Some('\\') | None => result.push('\\'),
                    Some(other) => {
                        // This shouldn't happen if lexer is correct
                        result.push('\\');
                        result.push(other);
                    }
                }
            } else {
                result.push(c);
            }
        }

        result
    }
}

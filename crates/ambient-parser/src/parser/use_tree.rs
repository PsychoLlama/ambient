//! Use/import parsing: Rust-style use trees.

use ambient_engine::ast::Span;

use super::Parser;
use crate::cst::{CstIdent, CstUseDef, CstUseTree, CstUseTreeKind, Trivia};
use crate::error::{ParseError, ParseErrorKind};
use crate::lexer::TokenKind;

impl Parser<'_> {
    /// Parse a use/import statement.
    ///
    /// Grammar (Rust-style use trees):
    /// ```text
    /// use_def   = ["pub"] "use" use_tree ";"
    /// use_tree  = "{" tree_list "}"
    ///           | segment ("::" segment)* tail
    /// tail      = "::" "{" tree_list "}"
    ///           | "as" ident
    ///           | ε
    /// tree_list = use_tree ("," use_tree)* [","]
    /// segment   = ident | "pkg" | "core" | "self" | "super"
    /// ```
    ///
    /// `as` is contextual (an identifier in alias position). Keyword
    /// segments are only valid at the head of a full path — validated
    /// during lowering, where the flattened paths exist.
    pub(super) fn parse_use(&mut self, is_public: bool) -> Result<CstUseDef, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::Use)?;
        let tree = self.parse_use_tree()?;
        let end = self.expect(TokenKind::Semi)?.span.end;
        Ok(CstUseDef {
            is_public,
            tree,
            span: Span::new(start, end),
        })
    }

    /// Parse one use-tree node (see [`Parser::parse_use`] for the grammar).
    fn parse_use_tree(&mut self) -> Result<CstUseTree, ParseError> {
        self.skip_trivia();
        let start = self.current().span.start;

        // Root brace group: `use {a::b, c};`
        if self.check(TokenKind::LBrace) {
            let group = self.parse_use_group()?;
            let end = self.current().span.start;
            return Ok(CstUseTree {
                segments: Vec::new(),
                kind: CstUseTreeKind::Group(group),
                span: Span::new(start, end),
            });
        }

        let mut segments = vec![self.parse_use_segment()?];
        loop {
            if self.consume(TokenKind::ColonColon).is_none() {
                break;
            }
            self.skip_trivia();
            if self.check(TokenKind::LBrace) {
                let group = self.parse_use_group()?;
                let end = self.current().span.start;
                return Ok(CstUseTree {
                    segments,
                    kind: CstUseTreeKind::Group(group),
                    span: Span::new(start, end),
                });
            }
            segments.push(self.parse_use_segment()?);
        }

        // Contextual `as` alias: `use a::b as c;`
        let alias = if self.check(TokenKind::Ident) && self.current().text == "as" {
            self.advance();
            self.skip_trivia();
            Some(self.parse_ident()?)
        } else {
            None
        };

        let end = self.current().span.start;
        Ok(CstUseTree {
            segments,
            kind: CstUseTreeKind::Leaf { alias },
            span: Span::new(start, end),
        })
    }

    /// Parse a brace group's contents: `{ tree, tree, }`.
    fn parse_use_group(&mut self) -> Result<Vec<CstUseTree>, ParseError> {
        self.expect(TokenKind::LBrace)?;
        let mut trees = Vec::new();
        loop {
            self.skip_trivia();
            if self.check(TokenKind::RBrace) {
                break;
            }
            trees.push(self.parse_use_tree()?);
            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }
        self.expect(TokenKind::RBrace)?;
        Ok(trees)
    }

    /// Parse one use-path segment: an identifier or a path-root keyword
    /// (`pkg`, `core`, `self`, `super`).
    pub(super) fn parse_use_segment(&mut self) -> Result<CstIdent, ParseError> {
        self.skip_trivia();
        match self.current_kind() {
            TokenKind::Ident => self.parse_ident(),
            TokenKind::Pkg | TokenKind::Core | TokenKind::Self_ | TokenKind::Super => {
                let token = self.current().clone();
                let ident = CstIdent {
                    name: token.text.into(),
                    span: token.span,
                    trailing_trivia: Trivia::default(),
                };
                self.advance();
                self.skip_trivia();
                Ok(ident)
            }
            _ => Err(ParseError::new(
                ParseErrorKind::Expected {
                    expected: "a path segment (identifier, pkg, core, self, or super)".into(),
                    found: format!("{:?}", self.current_kind()),
                },
                self.current().span,
            )),
        }
    }
}

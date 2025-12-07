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

mod expr;
mod patterns;
mod types;

use std::sync::Arc;

use ambient_engine::ast::Span;

use crate::cst::{
    CstAbilityDef, CstAbilityMethod, CstConstDef, CstEnumDef, CstEnumVariant, CstFunctionDef,
    CstIdent, CstItem, CstItemKind, CstModule, CstParam, CstQualifiedName, CstReplInput,
    CstTypeAliasDef, CstTypeParam, CstUseDef, CstUseImports, Trivia, TriviaItem, TriviaKind,
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
    /// # Panics
    ///
    /// Panics if lexing fails. Use [`Parser::try_new`] for fallible construction.
    #[must_use]
    pub fn new(source: &'src str) -> Self {
        Self::try_new(source).expect("lexer error")
    }

    /// Create a new parser for the given source, returning an error if lexing fails.
    ///
    /// This is the fallible version of [`Parser::new`].
    ///
    /// # Errors
    ///
    /// Returns a `ParseError` if the source contains invalid tokens (e.g., unterminated
    /// strings, invalid escape sequences).
    pub fn try_new(source: &'src str) -> Result<Self, ParseError> {
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
        while !self.at_end() {
            match self.current_kind() {
                TokenKind::Fn
                | TokenKind::Pub
                | TokenKind::Const
                | TokenKind::Type
                | TokenKind::Enum
                | TokenKind::Ability
                | TokenKind::Use => return,
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
        let start = self.current().span.start;
        let leading_trivia = self.skip_trivia();

        let mut items = Vec::new();
        // Skip trivia before checking at_end to handle trailing whitespace/newlines
        while {
            self.skip_trivia();
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

        // Return first error if any
        if let Some(e) = self.errors.first() {
            return Err(e.clone());
        }

        Ok(CstModule {
            name: "".into(), // Set by caller based on file path
            leading_trivia,
            items,
            trailing_trivia,
            span: Span::new(start, end),
        })
    }

    /// Parse a top-level item.
    fn parse_item(&mut self) -> Result<CstItem, ParseError> {
        let leading_trivia = self.skip_trivia();
        let start = self.current().span.start;

        let kind = match self.current_kind() {
            TokenKind::Pub => {
                self.advance();
                self.skip_trivia();
                match self.current_kind() {
                    TokenKind::Fn => CstItemKind::Function(self.parse_function(true)?),
                    _ => {
                        return Err(ParseError::new(
                            ParseErrorKind::Expected {
                                expected: "fn after pub".into(),
                                found: format!("{:?}", self.current_kind()),
                            },
                            self.current().span,
                        ));
                    }
                }
            }
            TokenKind::Fn => CstItemKind::Function(self.parse_function(false)?),
            TokenKind::Const => CstItemKind::Const(self.parse_const()?),
            TokenKind::Type | TokenKind::Unique => CstItemKind::TypeAlias(self.parse_type_alias()?),
            TokenKind::Enum => CstItemKind::Enum(self.parse_enum()?),
            TokenKind::Ability => CstItemKind::Ability(self.parse_ability_def()?),
            TokenKind::Use => CstItemKind::Use(self.parse_use()?),
            _ => {
                return Err(ParseError::new(
                    ParseErrorKind::UnexpectedToken(format!("{:?}", self.current_kind())),
                    self.current().span,
                ));
            }
        };

        let end = self.current().span.start;
        Ok(CstItem {
            leading_trivia,
            kind,
            span: Span::new(start, end),
        })
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
                | TokenKind::Unique
                | TokenKind::Enum
                | TokenKind::Ability
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
    // Function parsing
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_function(&mut self, is_public: bool) -> Result<CstFunctionDef, ParseError> {
        self.expect(TokenKind::Fn)?;

        let name = self.parse_ident()?;

        // Type parameters
        let type_params = if self.check(TokenKind::Lt) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };

        // Parameters
        self.expect(TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(TokenKind::RParen)?;

        // Return type
        let ret_ty = if self.consume(TokenKind::Colon).is_some() {
            Some(self.parse_type()?)
        } else {
            None
        };

        // Abilities
        let abilities = if self.check(TokenKind::With) {
            self.advance();
            self.parse_ability_list()?
        } else {
            Vec::new()
        };

        // Body
        let body = self.parse_block_expr()?;

        Ok(CstFunctionDef {
            is_public,
            name,
            type_params,
            params,
            ret_ty,
            abilities,
            body,
        })
    }

    fn parse_type_params(&mut self) -> Result<Vec<CstTypeParam>, ParseError> {
        self.expect(TokenKind::Lt)?;
        let mut params = Vec::new();

        loop {
            if self.check(TokenKind::Gt) {
                break;
            }

            let name = self.parse_ident()?;
            let is_ability = self.consume(TokenKind::Bang).is_some();
            let span = name.span;

            params.push(CstTypeParam {
                name,
                is_ability,
                span,
            });

            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }

        self.expect(TokenKind::Gt)?;
        Ok(params)
    }

    fn parse_params(&mut self) -> Result<Vec<CstParam>, ParseError> {
        let mut params = Vec::new();

        loop {
            if self.check(TokenKind::RParen) {
                break;
            }

            let name = self.parse_ident()?;
            let start = name.span.start;

            let ty = if self.consume(TokenKind::Colon).is_some() {
                Some(self.parse_type()?)
            } else {
                None
            };

            let end = ty.as_ref().map_or(name.span.end, |t| t.span.end);

            params.push(CstParam {
                name,
                ty,
                span: Span::new(start, end),
            });

            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }

        Ok(params)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Other definitions
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_const(&mut self) -> Result<CstConstDef, ParseError> {
        self.expect(TokenKind::Const)?;
        let name = self.parse_ident()?;
        self.expect(TokenKind::Colon)?;
        let ty = self.parse_type()?;
        self.expect(TokenKind::Eq)?;
        let value = self.parse_expression()?;
        self.expect(TokenKind::Semi)?;

        Ok(CstConstDef { name, ty, value })
    }

    fn parse_type_alias(&mut self) -> Result<CstTypeAliasDef, ParseError> {
        // Check for unique
        let unique_id = if self.consume(TokenKind::Unique).is_some() {
            self.expect(TokenKind::LParen)?;
            let id_token = self.advance();
            let id = id_token.text.clone();
            self.expect(TokenKind::RParen)?;
            Some(Arc::from(id.as_str()))
        } else {
            None
        };

        self.expect(TokenKind::Type)?;
        let name = self.parse_ident()?;

        let type_params = if self.check(TokenKind::Lt) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };

        // Could be `type Foo = Bar` or `type Foo { fields }`
        let ty = if self.consume(TokenKind::Eq).is_some() {
            let ty = self.parse_type()?;
            self.expect(TokenKind::Semi)?;
            ty
        } else {
            // Record type definition
            self.parse_record_type()?
        };

        Ok(CstTypeAliasDef {
            name,
            type_params,
            ty,
            unique_id,
        })
    }

    fn parse_enum(&mut self) -> Result<CstEnumDef, ParseError> {
        self.expect(TokenKind::Enum)?;
        let name = self.parse_ident()?;

        let type_params = if self.check(TokenKind::Lt) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };

        self.expect(TokenKind::LBrace)?;

        let mut variants = Vec::new();
        loop {
            if self.check(TokenKind::RBrace) {
                break;
            }

            let variant_name = self.parse_ident()?;
            let start = variant_name.span.start;

            let payload = if self.consume(TokenKind::LParen).is_some() {
                let ty = self.parse_type()?;
                self.expect(TokenKind::RParen)?;
                Some(ty)
            } else {
                None
            };

            let end = payload
                .as_ref()
                .map_or(variant_name.span.end, |t| t.span.end);

            variants.push(CstEnumVariant {
                name: variant_name,
                payload,
                span: Span::new(start, end),
            });

            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }

        self.expect(TokenKind::RBrace)?;

        Ok(CstEnumDef {
            name,
            type_params,
            variants,
        })
    }

    fn parse_ability_def(&mut self) -> Result<CstAbilityDef, ParseError> {
        self.expect(TokenKind::Ability)?;
        let name = self.parse_ident()?;

        // Dependencies
        let dependencies = if self.check(TokenKind::With) {
            self.advance();
            self.parse_ability_list()?
        } else {
            Vec::new()
        };

        self.expect(TokenKind::LBrace)?;

        let mut methods = Vec::new();
        loop {
            if self.check(TokenKind::RBrace) {
                break;
            }

            methods.push(self.parse_ability_method()?);
        }

        self.expect(TokenKind::RBrace)?;

        Ok(CstAbilityDef {
            name,
            dependencies,
            methods,
        })
    }

    fn parse_ability_method(&mut self) -> Result<CstAbilityMethod, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::Fn)?;
        let name = self.parse_ident()?;

        let type_params = if self.check(TokenKind::Lt) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };

        self.expect(TokenKind::LParen)?;

        let mut params = Vec::new();
        loop {
            if self.check(TokenKind::RParen) {
                break;
            }

            let param_name = self.parse_ident()?;
            self.expect(TokenKind::Colon)?;
            let param_ty = self.parse_type()?;

            params.push((param_name, param_ty));

            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }

        self.expect(TokenKind::RParen)?;
        self.expect(TokenKind::Colon)?;
        let ret_ty = self.parse_type()?;
        let end = self.expect(TokenKind::Semi)?.span.end;

        Ok(CstAbilityMethod {
            name,
            type_params,
            params,
            ret_ty,
            span: Span::new(start, end),
        })
    }

    fn parse_use(&mut self) -> Result<CstUseDef, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::Use)?;

        let mut path = Vec::new();
        path.push(self.parse_ident()?);

        while self.consume(TokenKind::Dot).is_some() {
            // Check for glob or nested imports
            if self.consume(TokenKind::Star).is_some() {
                let end = self.expect(TokenKind::Semi)?.span.end;
                return Ok(CstUseDef {
                    path,
                    imports: CstUseImports::All,
                    span: Span::new(start, end),
                });
            }

            if self.check(TokenKind::LBrace) {
                self.advance();
                let mut items = Vec::new();

                loop {
                    if self.check(TokenKind::RBrace) {
                        break;
                    }

                    items.push(self.parse_ident()?);

                    if self.consume(TokenKind::Comma).is_none() {
                        break;
                    }
                }

                self.expect(TokenKind::RBrace)?;
                let end = self.expect(TokenKind::Semi)?.span.end;

                return Ok(CstUseDef {
                    path,
                    imports: CstUseImports::Items(items),
                    span: Span::new(start, end),
                });
            }

            path.push(self.parse_ident()?);
        }

        let end = self.expect(TokenKind::Semi)?.span.end;
        Ok(CstUseDef {
            path,
            imports: CstUseImports::Single,
            span: Span::new(start, end),
        })
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

        segments.push(self.parse_ident()?);

        while self.consume(TokenKind::Dot).is_some() {
            if !self.check(TokenKind::Ident) {
                break;
            }
            segments.push(self.parse_ident()?);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cst::{CstBinaryOp, CstExprKind, CstUnaryOp};

    #[test]
    fn test_parse_number() {
        let mut parser = Parser::new("42");
        let expr = parser.parse_expression().expect("parse error");
        assert!(matches!(expr.kind, CstExprKind::Number(n) if n == 42.0));
    }

    #[test]
    fn test_parse_string() {
        let mut parser = Parser::new(r#""hello""#);
        let expr = parser.parse_expression().expect("parse error");
        assert!(matches!(&expr.kind, CstExprKind::String(s) if &**s == "hello"));
    }

    #[test]
    fn test_parse_bool() {
        let mut parser = Parser::new("true");
        let expr = parser.parse_expression().expect("parse error");
        assert!(matches!(expr.kind, CstExprKind::Bool(true)));

        let mut parser = Parser::new("false");
        let expr = parser.parse_expression().expect("parse error");
        assert!(matches!(expr.kind, CstExprKind::Bool(false)));
    }

    #[test]
    fn test_parse_binary_expr() {
        let mut parser = Parser::new("1 + 2 * 3");
        let expr = parser.parse_expression().expect("parse error");

        // Should parse as 1 + (2 * 3)
        match expr.kind {
            CstExprKind::Binary { op, left, right } => {
                assert_eq!(op, CstBinaryOp::Add);
                assert!(matches!(left.kind, CstExprKind::Number(n) if n == 1.0));
                match right.kind {
                    CstExprKind::Binary { op, left, right } => {
                        assert_eq!(op, CstBinaryOp::Mul);
                        assert!(matches!(left.kind, CstExprKind::Number(n) if n == 2.0));
                        assert!(matches!(right.kind, CstExprKind::Number(n) if n == 3.0));
                    }
                    _ => panic!("Expected binary expression"),
                }
            }
            _ => panic!("Expected binary expression"),
        }
    }

    #[test]
    fn test_parse_unary_expr() {
        let mut parser = Parser::new("-42");
        let expr = parser.parse_expression().expect("parse error");
        match expr.kind {
            CstExprKind::Unary { op, operand } => {
                assert_eq!(op, CstUnaryOp::Neg);
                assert!(matches!(operand.kind, CstExprKind::Number(n) if n == 42.0));
            }
            _ => panic!("Expected unary expression"),
        }
    }

    #[test]
    fn test_parse_if_expr() {
        let mut parser = Parser::new("if x { 1 } else { 2 }");
        let expr = parser.parse_expression().expect("parse error");
        assert!(matches!(expr.kind, CstExprKind::If { .. }));
    }

    #[test]
    fn test_parse_lambda() {
        let mut parser = Parser::new("(x) => x + 1");
        let expr = parser.parse_expression().expect("parse error");
        match expr.kind {
            CstExprKind::Lambda(lambda) => {
                assert_eq!(lambda.params.len(), 1);
                assert_eq!(&*lambda.params[0].name.name, "x");
            }
            _ => panic!("Expected lambda"),
        }
    }

    #[test]
    fn test_parse_function() {
        let source = "fn add(x: number, y: number): number { x + y }";
        let mut parser = Parser::new(source);
        let module = parser.parse_module().expect("parse error");
        assert_eq!(module.items.len(), 1);
        match &module.items[0].kind {
            CstItemKind::Function(f) => {
                assert_eq!(&*f.name.name, "add");
                assert!(!f.is_public);
                assert_eq!(f.params.len(), 2);
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_parse_pub_function() {
        let source = "pub fn main(): () { () }";
        let mut parser = Parser::new(source);
        let module = parser.parse_module().expect("parse error");
        assert_eq!(module.items.len(), 1);
        match &module.items[0].kind {
            CstItemKind::Function(f) => {
                assert_eq!(&*f.name.name, "main");
                assert!(f.is_public);
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_parse_function_with_abilities() {
        let source = "fn read_file(path: string): string with Filesystem { path }";
        let mut parser = Parser::new(source);
        let module = parser.parse_module().expect("parse error");
        match &module.items[0].kind {
            CstItemKind::Function(f) => {
                assert_eq!(f.abilities.len(), 1);
                assert_eq!(&*f.abilities[0].segments[0].name, "Filesystem");
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_parse_enum() {
        let source = "enum Option<T> { Some(T), None }";
        let mut parser = Parser::new(source);
        let module = parser.parse_module().expect("parse error");
        match &module.items[0].kind {
            CstItemKind::Enum(e) => {
                assert_eq!(&*e.name.name, "Option");
                assert_eq!(e.type_params.len(), 1);
                assert_eq!(e.variants.len(), 2);
            }
            _ => panic!("Expected enum"),
        }
    }

    #[test]
    fn test_parse_ability_def() {
        let source = "ability Console { fn print(message: string): (); }";
        let mut parser = Parser::new(source);
        let module = parser.parse_module().expect("parse error");
        match &module.items[0].kind {
            CstItemKind::Ability(a) => {
                assert_eq!(&*a.name.name, "Console");
                assert_eq!(a.methods.len(), 1);
                assert_eq!(&*a.methods[0].name.name, "print");
            }
            _ => panic!("Expected ability"),
        }
    }

    #[test]
    fn test_parse_record_literal() {
        let mut parser = Parser::new("{ x: 1, y: 2 }");
        let expr = parser.parse_expression().expect("parse error");
        match expr.kind {
            CstExprKind::Record(fields) => {
                assert_eq!(fields.len(), 2);
                assert_eq!(&*fields[0].0.name, "x");
                assert_eq!(&*fields[1].0.name, "y");
            }
            _ => panic!("Expected record"),
        }
    }

    #[test]
    fn test_parse_list_literal() {
        let mut parser = Parser::new("[1, 2, 3]");
        let expr = parser.parse_expression().expect("parse error");
        match expr.kind {
            CstExprKind::List(elements) => {
                assert_eq!(elements.len(), 3);
            }
            _ => panic!("Expected list"),
        }
    }

    #[test]
    fn test_parse_match() {
        let source = r#"
            match x {
                Some(v) => v,
                None => 0,
            }
        "#;
        let mut parser = Parser::new(source);
        let expr = parser.parse_expression().expect("parse error");
        match expr.kind {
            CstExprKind::Match { arms, .. } => {
                assert_eq!(arms.len(), 2);
            }
            _ => panic!("Expected match"),
        }
    }

    #[test]
    fn test_parse_tuple() {
        let mut parser = Parser::new("(1, 2, 3)");
        let expr = parser.parse_expression().expect("parse error");
        match expr.kind {
            CstExprKind::Tuple(elements) => {
                assert_eq!(elements.len(), 3);
            }
            _ => panic!("Expected tuple"),
        }
    }

    #[test]
    fn test_parse_unit() {
        let mut parser = Parser::new("()");
        let expr = parser.parse_expression().expect("parse error");
        assert!(matches!(expr.kind, CstExprKind::Unit));
    }

    #[test]
    fn test_parse_block() {
        let source = r#"
            {
                let x = 1;
                let y = 2;
                x + y
            }
        "#;
        let mut parser = Parser::new(source);
        let expr = parser.parse_expression().expect("parse error");
        match expr.kind {
            CstExprKind::Block { stmts, result } => {
                assert_eq!(stmts.len(), 2);
                assert!(result.is_some());
            }
            _ => panic!("Expected block"),
        }
    }

    #[test]
    fn test_parse_handler_literal() {
        let source = r#"
            {
                read(path) => resume("mock content"),
                write(path, content) => resume(())
            }
        "#;
        let mut parser = Parser::new(source);
        let expr = parser.parse_expression().expect("parse error");
        match expr.kind {
            CstExprKind::HandlerLiteral(handler_lit) => {
                assert_eq!(handler_lit.methods.len(), 2);

                // Check first method
                assert_eq!(&*handler_lit.methods[0].method.name, "read");
                assert_eq!(handler_lit.methods[0].params.len(), 1);
                assert_eq!(&*handler_lit.methods[0].params[0].name.name, "path");

                // Check second method
                assert_eq!(&*handler_lit.methods[1].method.name, "write");
                assert_eq!(handler_lit.methods[1].params.len(), 2);
                assert_eq!(&*handler_lit.methods[1].params[0].name.name, "path");
                assert_eq!(&*handler_lit.methods[1].params[1].name.name, "content");
            }
            _ => panic!("Expected handler literal, got {:?}", expr.kind),
        }
    }

    #[test]
    fn test_parse_handler_literal_single_method() {
        let source = r#"{ print(msg) => resume(()) }"#;
        let mut parser = Parser::new(source);
        let expr = parser.parse_expression().expect("parse error");
        match expr.kind {
            CstExprKind::HandlerLiteral(handler_lit) => {
                assert_eq!(handler_lit.methods.len(), 1);
                assert_eq!(&*handler_lit.methods[0].method.name, "print");
            }
            _ => panic!("Expected handler literal"),
        }
    }

    #[test]
    fn test_parse_handler_literal_empty_params() {
        let source = r#"{ now() => resume(42) }"#;
        let mut parser = Parser::new(source);
        let expr = parser.parse_expression().expect("parse error");
        match expr.kind {
            CstExprKind::HandlerLiteral(handler_lit) => {
                assert_eq!(handler_lit.methods.len(), 1);
                assert_eq!(&*handler_lit.methods[0].method.name, "now");
                assert!(handler_lit.methods[0].params.is_empty());
            }
            _ => panic!("Expected handler literal"),
        }
    }

    #[test]
    fn test_parse_sandbox_with_abilities() {
        let source = r#"sandbox with Log, Console { untrusted_code() }"#;
        let mut parser = Parser::new(source);
        let expr = parser.parse_expression().expect("parse error");
        match expr.kind {
            CstExprKind::Sandbox(sandbox) => {
                assert_eq!(sandbox.allowed_abilities.len(), 2);
                assert_eq!(&*sandbox.allowed_abilities[0].segments[0].name, "Log");
                assert_eq!(&*sandbox.allowed_abilities[1].segments[0].name, "Console");
            }
            _ => panic!("Expected sandbox expression"),
        }
    }

    #[test]
    fn test_parse_sandbox_pure() {
        let source = r#"sandbox { pure_computation() }"#;
        let mut parser = Parser::new(source);
        let expr = parser.parse_expression().expect("parse error");
        match expr.kind {
            CstExprKind::Sandbox(sandbox) => {
                assert!(sandbox.allowed_abilities.is_empty());
            }
            _ => panic!("Expected sandbox expression"),
        }
    }

    #[test]
    fn test_parse_sandbox_single_ability() {
        let source = r#"sandbox with Log { plugin() }"#;
        let mut parser = Parser::new(source);
        let expr = parser.parse_expression().expect("parse error");
        match expr.kind {
            CstExprKind::Sandbox(sandbox) => {
                assert_eq!(sandbox.allowed_abilities.len(), 1);
                assert_eq!(&*sandbox.allowed_abilities[0].segments[0].name, "Log");
            }
            _ => panic!("Expected sandbox expression"),
        }
    }
}

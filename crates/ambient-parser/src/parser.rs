//! Recursive descent parser for the Ambient language.
//!
//! The parser consumes tokens from the lexer and builds a CST.
//! It implements error recovery to continue parsing after errors.

// Allow expect() for internal invariants (e.g., segments.last() on non-empty vectors)
#![allow(clippy::expect_used)]

use std::sync::Arc;

use ambient_engine::ast::Span;

use crate::cst::{
    CstAbilityDef, CstAbilityMethod, CstBinaryOp, CstConstDef, CstEnumDef, CstEnumVariant,
    CstExpr, CstExprKind, CstFunctionDef, CstHandler, CstHandleExpr, CstHandlerLiteralExpr,
    CstHandlerLiteralMethod, CstIdent, CstItem, CstItemKind, CstLambda, CstLetBinding,
    CstLiteral, CstMatchArm, CstModule, CstParam, CstPattern, CstPatternKind, CstQualifiedName,
    CstRecordPatternField, CstSandboxExpr, CstStmt, CstStmtKind, CstTypeAliasDef, CstTypeExpr,
    CstTypeExprKind, CstTypeParam, CstUnaryOp, CstUseDef, CstUseImports, StringPart, Trivia,
    TriviaItem, TriviaKind,
};
use crate::error::{ParseError, ParseErrorKind};
use crate::lexer::{Lexer, Token, TokenKind};

/// The parser for Ambient source code.
pub struct Parser<'src> {
    #[allow(dead_code)]
    source: &'src str,
    tokens: Vec<Token>,
    pos: usize,
    /// Collected errors for error recovery.
    errors: Vec<ParseError>,
}

impl<'src> Parser<'src> {
    /// Create a new parser for the given source.
    ///
    /// # Panics
    ///
    /// Panics if lexing fails (use `Parser::try_new` for fallible construction).
    #[must_use]
    pub fn new(source: &'src str) -> Self {
        let mut lexer = Lexer::new(source);
        // For now, panic on lex errors. In a full implementation,
        // we'd want to recover and continue.
        let tokens = lexer.tokenize().expect("lexer error");
        Self {
            source,
            tokens,
            pos: 0,
            errors: Vec::new(),
        }
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

    fn parse_ability_list(&mut self) -> Result<Vec<CstQualifiedName>, ParseError> {
        let mut abilities = Vec::new();

        loop {
            abilities.push(self.parse_qualified_name()?);

            if self.consume(TokenKind::Comma).is_none() {
                break;
            }

            // Check for trailing comma before block
            if self.check(TokenKind::LBrace) {
                break;
            }
        }

        Ok(abilities)
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

    fn parse_record_type(&mut self) -> Result<CstTypeExpr, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::LBrace)?;

        let mut fields = Vec::new();
        loop {
            if self.check(TokenKind::RBrace) {
                break;
            }

            let field_name = self.parse_ident()?;
            self.expect(TokenKind::Colon)?;
            let field_ty = self.parse_type()?;

            fields.push((field_name, field_ty));

            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }

        let end = self.expect(TokenKind::RBrace)?.span.end;

        Ok(CstTypeExpr {
            kind: CstTypeExprKind::Record(fields),
            span: Span::new(start, end),
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

            let end = payload.as_ref().map_or(variant_name.span.end, |t| t.span.end);

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
    // Type parsing
    // ─────────────────────────────────────────────────────────────────────────

    /// Parse a type expression.
    pub fn parse_type(&mut self) -> Result<CstTypeExpr, ParseError> {
        let start = self.current().span.start;

        // Check for special types
        if self.consume(TokenKind::Bang).is_some() {
            return Ok(CstTypeExpr {
                kind: CstTypeExprKind::Never,
                span: Span::new(start, self.current().span.start),
            });
        }

        if self.consume(TokenKind::Underscore).is_some() {
            return Ok(CstTypeExpr {
                kind: CstTypeExprKind::Infer,
                span: Span::new(start, self.current().span.start),
            });
        }

        // Tuple or parenthesized type
        if self.check(TokenKind::LParen) {
            return self.parse_tuple_or_function_type();
        }

        // Record type
        if self.check(TokenKind::LBrace) {
            return self.parse_record_type();
        }

        // Named type (possibly generic)
        let base = self.parse_qualified_name()?;
        let base_span = base.span;

        let base_ty = CstTypeExpr {
            kind: CstTypeExprKind::Name(base),
            span: base_span,
        };

        // Check for generic arguments
        if self.check(TokenKind::Lt) {
            self.advance();
            let mut args = Vec::new();

            loop {
                if self.check(TokenKind::Gt) {
                    break;
                }

                args.push(self.parse_type()?);

                if self.consume(TokenKind::Comma).is_none() {
                    break;
                }
            }

            let end = self.expect(TokenKind::Gt)?.span.end;

            return Ok(CstTypeExpr {
                kind: CstTypeExprKind::Generic {
                    base: Box::new(base_ty),
                    args,
                },
                span: Span::new(start, end),
            });
        }

        Ok(base_ty)
    }

    fn parse_tuple_or_function_type(&mut self) -> Result<CstTypeExpr, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::LParen)?;

        let mut elements = Vec::new();
        loop {
            if self.check(TokenKind::RParen) {
                break;
            }

            elements.push(self.parse_type()?);

            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }

        self.expect(TokenKind::RParen)?;

        // Check for function type
        if self.consume(TokenKind::Arrow).is_some() {
            let ret = self.parse_type()?;
            let abilities = if self.check(TokenKind::With) {
                self.advance();
                self.parse_ability_list()?
            } else {
                Vec::new()
            };

            let end = ret.span.end;

            return Ok(CstTypeExpr {
                kind: CstTypeExprKind::Function {
                    params: elements,
                    ret: Box::new(ret),
                    abilities,
                },
                span: Span::new(start, end),
            });
        }

        let end = self.current().span.start;

        // Unit type or tuple type
        Ok(CstTypeExpr {
            kind: CstTypeExprKind::Tuple(elements),
            span: Span::new(start, end),
        })
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Expression parsing
    // ─────────────────────────────────────────────────────────────────────────

    /// Parse an expression.
    pub fn parse_expression(&mut self) -> Result<CstExpr, ParseError> {
        self.parse_or_expr()
    }

    fn parse_or_expr(&mut self) -> Result<CstExpr, ParseError> {
        let mut left = self.parse_and_expr()?;

        while self.consume(TokenKind::OrOr).is_some() {
            let right = self.parse_and_expr()?;
            let span = Span::new(left.span.start, right.span.end);
            left = CstExpr {
                kind: CstExprKind::Binary {
                    op: CstBinaryOp::Or,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            };
        }

        Ok(left)
    }

    fn parse_and_expr(&mut self) -> Result<CstExpr, ParseError> {
        let mut left = self.parse_equality_expr()?;

        while self.consume(TokenKind::AndAnd).is_some() {
            let right = self.parse_equality_expr()?;
            let span = Span::new(left.span.start, right.span.end);
            left = CstExpr {
                kind: CstExprKind::Binary {
                    op: CstBinaryOp::And,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            };
        }

        Ok(left)
    }

    fn parse_equality_expr(&mut self) -> Result<CstExpr, ParseError> {
        let mut left = self.parse_comparison_expr()?;

        loop {
            let op = if self.consume(TokenKind::EqEq).is_some() {
                CstBinaryOp::Eq
            } else if self.consume(TokenKind::Ne).is_some() {
                CstBinaryOp::Ne
            } else {
                break;
            };

            let right = self.parse_comparison_expr()?;
            let span = Span::new(left.span.start, right.span.end);
            left = CstExpr {
                kind: CstExprKind::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            };
        }

        Ok(left)
    }

    fn parse_comparison_expr(&mut self) -> Result<CstExpr, ParseError> {
        let mut left = self.parse_additive_expr()?;

        loop {
            let op = if self.consume(TokenKind::Lt).is_some() {
                CstBinaryOp::Lt
            } else if self.consume(TokenKind::Le).is_some() {
                CstBinaryOp::Le
            } else if self.consume(TokenKind::Gt).is_some() {
                CstBinaryOp::Gt
            } else if self.consume(TokenKind::Ge).is_some() {
                CstBinaryOp::Ge
            } else {
                break;
            };

            let right = self.parse_additive_expr()?;
            let span = Span::new(left.span.start, right.span.end);
            left = CstExpr {
                kind: CstExprKind::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            };
        }

        Ok(left)
    }

    fn parse_additive_expr(&mut self) -> Result<CstExpr, ParseError> {
        let mut left = self.parse_multiplicative_expr()?;

        loop {
            let op = if self.consume(TokenKind::Plus).is_some() {
                CstBinaryOp::Add
            } else if self.consume(TokenKind::Minus).is_some() {
                CstBinaryOp::Sub
            } else {
                break;
            };

            let right = self.parse_multiplicative_expr()?;
            let span = Span::new(left.span.start, right.span.end);
            left = CstExpr {
                kind: CstExprKind::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            };
        }

        Ok(left)
    }

    fn parse_multiplicative_expr(&mut self) -> Result<CstExpr, ParseError> {
        let mut left = self.parse_unary_expr()?;

        loop {
            let op = if self.consume(TokenKind::Star).is_some() {
                CstBinaryOp::Mul
            } else if self.consume(TokenKind::Slash).is_some() {
                CstBinaryOp::Div
            } else if self.consume(TokenKind::Percent).is_some() {
                CstBinaryOp::Mod
            } else {
                break;
            };

            let right = self.parse_unary_expr()?;
            let span = Span::new(left.span.start, right.span.end);
            left = CstExpr {
                kind: CstExprKind::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            };
        }

        Ok(left)
    }

    fn parse_unary_expr(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;

        if self.consume(TokenKind::Minus).is_some() {
            let operand = self.parse_unary_expr()?;
            let span = Span::new(start, operand.span.end);
            return Ok(CstExpr {
                kind: CstExprKind::Unary {
                    op: CstUnaryOp::Neg,
                    operand: Box::new(operand),
                },
                span,
            });
        }

        if self.consume(TokenKind::Bang).is_some() {
            let operand = self.parse_unary_expr()?;
            let span = Span::new(start, operand.span.end);
            return Ok(CstExpr {
                kind: CstExprKind::Unary {
                    op: CstUnaryOp::Not,
                    operand: Box::new(operand),
                },
                span,
            });
        }

        self.parse_postfix_expr()
    }

    fn parse_postfix_expr(&mut self) -> Result<CstExpr, ParseError> {
        let mut expr = self.parse_primary_expr()?;

        loop {
            if self.consume(TokenKind::Dot).is_some() {
                // Field access or tuple index
                if self.check(TokenKind::Number) {
                    let token = self.advance();
                    let index: u32 = token
                        .text
                        .parse()
                        .map_err(|_| ParseError::new(ParseErrorKind::InvalidExpression, token.span))?;
                    let span = Span::new(expr.span.start, token.span.end);
                    expr = CstExpr {
                        kind: CstExprKind::TupleIndex {
                            tuple: Box::new(expr),
                            index,
                        },
                        span,
                    };
                } else {
                    let field = self.parse_ident()?;

                    // Check for ability method call
                    if self.check(TokenKind::Bang) || self.check(TokenKind::LParen) {
                        let is_perform = self.consume(TokenKind::Bang).is_some();
                        self.expect(TokenKind::LParen)?;
                        let args = self.parse_args()?;
                        let end = self.expect(TokenKind::RParen)?.span.end;

                        // Reconstruct ability name from the expression
                        let ability = match &expr.kind {
                            CstExprKind::Ident(ident) => CstQualifiedName {
                                segments: vec![ident.clone()],
                                span: expr.span,
                            },
                            CstExprKind::QualifiedName(qn) => qn.clone(),
                            _ => {
                                return Err(ParseError::new(
                                    ParseErrorKind::InvalidAbilitySyntax(
                                        "expected ability name before method".into(),
                                    ),
                                    expr.span,
                                ));
                            }
                        };

                        let span = Span::new(expr.span.start, end);
                        expr = if is_perform {
                            CstExpr {
                                kind: CstExprKind::Perform {
                                    ability,
                                    method: field,
                                    args,
                                },
                                span,
                            }
                        } else {
                            CstExpr {
                                kind: CstExprKind::Suspend {
                                    ability,
                                    method: field,
                                    args,
                                },
                                span,
                            }
                        };
                    } else {
                        let span = Span::new(expr.span.start, field.span.end);
                        expr = CstExpr {
                            kind: CstExprKind::Field {
                                record: Box::new(expr),
                                field,
                            },
                            span,
                        };
                    }
                }
            } else if self.check(TokenKind::LParen) {
                // Function call
                self.advance();
                let args = self.parse_args()?;
                let end = self.expect(TokenKind::RParen)?.span.end;
                let span = Span::new(expr.span.start, end);
                expr = CstExpr {
                    kind: CstExprKind::Call {
                        callee: Box::new(expr),
                        args,
                    },
                    span,
                };
            } else if self.consume(TokenKind::Bang).is_some() {
                // Standalone perform on suspended ability
                // This handles: `suspended_value!`
                let span = Span::new(expr.span.start, self.current().span.start);

                // Check if this is an ability method call pattern
                // For pattern like `Ability.method!(args)`, we would have already
                // handled it above. This branch handles `value!` where value
                // is a suspended ability.
                match &expr.kind {
                    CstExprKind::Field { record, field } => {
                        // Convert Field + Bang into Perform
                        let ability = match &record.kind {
                            CstExprKind::Ident(ident) => CstQualifiedName {
                                segments: vec![ident.clone()],
                                span: record.span,
                            },
                            CstExprKind::QualifiedName(qn) => qn.clone(),
                            _ => {
                                return Err(ParseError::new(
                                    ParseErrorKind::InvalidAbilitySyntax(
                                        "expected ability name".into(),
                                    ),
                                    record.span,
                                ));
                            }
                        };

                        // Expect arguments
                        self.expect(TokenKind::LParen)?;
                        let args = self.parse_args()?;
                        let end = self.expect(TokenKind::RParen)?.span.end;

                        expr = CstExpr {
                            kind: CstExprKind::Perform {
                                ability,
                                method: field.clone(),
                                args,
                            },
                            span: Span::new(record.span.start, end),
                        };
                    }
                    _ => {
                        // Generic perform on a value - wrap it
                        // For now, treat as error since we need proper ability call syntax
                        return Err(ParseError::new(
                            ParseErrorKind::InvalidAbilitySyntax(
                                "standalone ! requires ability.method! syntax".into(),
                            ),
                            span,
                        ));
                    }
                }
            } else {
                break;
            }
        }

        Ok(expr)
    }

    fn parse_primary_expr(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;

        // Literals
        if self.check(TokenKind::True) {
            self.advance();
            return Ok(CstExpr {
                kind: CstExprKind::Bool(true),
                span: Span::new(start, self.current().span.start),
            });
        }

        if self.check(TokenKind::False) {
            self.advance();
            return Ok(CstExpr {
                kind: CstExprKind::Bool(false),
                span: Span::new(start, self.current().span.start),
            });
        }

        if self.check(TokenKind::Number) {
            let token = self.advance();
            let value: f64 = token
                .text
                .parse()
                .map_err(|_| ParseError::new(ParseErrorKind::InvalidNumber(token.text.clone()), token.span))?;
            return Ok(CstExpr {
                kind: CstExprKind::Number(value),
                span: token.span,
            });
        }

        if self.check(TokenKind::String) {
            let token = self.advance();
            let value = self.unescape_string(&token.text)?;
            return Ok(CstExpr {
                kind: CstExprKind::String(value.into()),
                span: token.span,
            });
        }

        // Interpolated string
        if self.check(TokenKind::StringStart) {
            return self.parse_interpolated_string();
        }

        // Parenthesized expression, tuple, or lambda
        if self.check(TokenKind::LParen) {
            return self.parse_paren_expr();
        }

        // List literal
        if self.check(TokenKind::LBracket) {
            return self.parse_list_expr();
        }

        // Block or record literal
        if self.check(TokenKind::LBrace) {
            return self.parse_brace_expr();
        }

        // If expression
        if self.check(TokenKind::If) {
            return self.parse_if_expr();
        }

        // Match expression
        if self.check(TokenKind::Match) {
            return self.parse_match_expr();
        }

        // Handle expression
        if self.check(TokenKind::Handle) {
            return self.parse_handle_expr();
        }

        // Resume expression: resume(value)
        if self.check(TokenKind::Resume) {
            return self.parse_resume_expr();
        }

        // Sandbox expression: sandbox with Ability { ... } or sandbox { ... }
        if self.check(TokenKind::Sandbox) {
            return self.parse_sandbox_expr();
        }

        // Identifier or qualified name
        if self.check(TokenKind::Ident) {
            let ident = self.parse_ident()?;

            // Check for qualified name
            if self.check(TokenKind::Dot) {
                let mut segments = vec![ident];
                while self.consume(TokenKind::Dot).is_some() {
                    if !self.check(TokenKind::Ident) && !self.check(TokenKind::Number) {
                        break;
                    }
                    if self.check(TokenKind::Ident) {
                        segments.push(self.parse_ident()?);
                    } else {
                        // Tuple index - backtrack
                        // Put the dot back conceptually by returning what we have
                        break;
                    }
                }

                if segments.len() > 1 {
                    let span = Span::new(segments[0].span.start, segments.last().expect("segments not empty").span.end);
                    return Ok(CstExpr {
                        kind: CstExprKind::QualifiedName(CstQualifiedName { segments, span }),
                        span,
                    });
                }
                // Only one segment, return as ident
                return Ok(CstExpr {
                    kind: CstExprKind::Ident(segments.into_iter().next().expect("segments not empty")),
                    span: Span::new(start, self.current().span.start),
                });
            }

            let span = ident.span;
            return Ok(CstExpr {
                kind: CstExprKind::Ident(ident),
                span,
            });
        }

        Err(ParseError::new(
            ParseErrorKind::UnexpectedToken(format!("{:?}", self.current_kind())),
            self.current().span,
        ))
    }

    fn parse_interpolated_string(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;
        let mut parts = Vec::new();

        // First part (StringStart)
        let token = self.advance();
        if !token.text.is_empty() {
            let content = self.unescape_string_part(&token.text)?;
            parts.push(StringPart::Literal(content.into(), token.span));
        }

        loop {
            // Parse interpolated expression
            let expr = self.parse_expression()?;
            parts.push(StringPart::Expr(expr));

            // Check what comes next
            match self.current_kind() {
                TokenKind::StringMiddle => {
                    let token = self.advance();
                    if !token.text.is_empty() {
                        let content = self.unescape_string_part(&token.text)?;
                        parts.push(StringPart::Literal(content.into(), token.span));
                    }
                }
                TokenKind::StringEnd => {
                    let token = self.advance();
                    if !token.text.is_empty() {
                        let content = self.unescape_string_part(&token.text)?;
                        parts.push(StringPart::Literal(content.into(), token.span));
                    }
                    break;
                }
                _ => {
                    return Err(ParseError::new(
                        ParseErrorKind::UnterminatedInterpolation,
                        self.current().span,
                    ));
                }
            }
        }

        let end = self.current().span.start;
        Ok(CstExpr {
            kind: CstExprKind::InterpolatedString(parts),
            span: Span::new(start, end),
        })
    }

    fn parse_paren_expr(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::LParen)?;

        // Empty parens = unit
        if self.check(TokenKind::RParen) {
            self.advance();
            return Ok(CstExpr {
                kind: CstExprKind::Unit,
                span: Span::new(start, self.current().span.start),
            });
        }

        // Check for lambda: (params) => body
        // We need to peek ahead to see if this looks like a lambda
        if self.is_lambda_start() {
            return self.parse_lambda(start);
        }

        // Parse first expression
        let first = self.parse_expression()?;

        // Single expression in parens
        if self.check(TokenKind::RParen) {
            self.advance();
            return Ok(first);
        }

        // Tuple
        if self.consume(TokenKind::Comma).is_some() {
            let mut elements = vec![first];

            loop {
                if self.check(TokenKind::RParen) {
                    break;
                }

                elements.push(self.parse_expression()?);

                if self.consume(TokenKind::Comma).is_none() {
                    break;
                }
            }

            let end = self.expect(TokenKind::RParen)?.span.end;
            return Ok(CstExpr {
                kind: CstExprKind::Tuple(elements),
                span: Span::new(start, end),
            });
        }

        self.expect(TokenKind::RParen)?;
        Ok(first)
    }

    fn is_lambda_start(&mut self) -> bool {
        // Save position
        let saved_pos = self.pos;

        // Skip trivia and look ahead
        self.skip_trivia();

        // Lambda patterns:
        // () => ...
        // (x) => ...
        // (x, y) => ...
        // (x: Type) => ...

        let result = self.peek_for_lambda();

        // Restore position
        self.pos = saved_pos;
        result
    }

    fn peek_for_lambda(&mut self) -> bool {
        // We're at the opening paren
        let mut depth = 1;
        let saved = self.pos;

        self.advance(); // consume (

        while depth > 0 && !self.at_end() {
            self.skip_trivia();
            match self.current_kind() {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => depth -= 1,
                _ => {}
            }
            self.advance();
        }

        self.skip_trivia();
        let is_arrow = self.check(TokenKind::FatArrow);
        self.pos = saved;
        is_arrow
    }

    fn parse_lambda(&mut self, start: u32) -> Result<CstExpr, ParseError> {
        // Parse parameters
        let params = self.parse_params()?;
        self.expect(TokenKind::RParen)?;

        // Optional return type
        let ret_ty = if self.consume(TokenKind::Colon).is_some() {
            Some(self.parse_type()?)
        } else {
            None
        };

        self.expect(TokenKind::FatArrow)?;

        // Body can be a block or a single expression
        let body = if self.check(TokenKind::LBrace) {
            self.parse_block_expr()?
        } else {
            self.parse_expression()?
        };

        let end = body.span.end;
        let span = Span::new(start, end);

        Ok(CstExpr {
            kind: CstExprKind::Lambda(CstLambda {
                params,
                ret_ty,
                body: Box::new(body),
                span,
            }),
            span,
        })
    }

    fn parse_list_expr(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::LBracket)?;

        let mut elements = Vec::new();
        loop {
            if self.check(TokenKind::RBracket) {
                break;
            }

            elements.push(self.parse_expression()?);

            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }

        let end = self.expect(TokenKind::RBracket)?.span.end;
        Ok(CstExpr {
            kind: CstExprKind::List(elements),
            span: Span::new(start, end),
        })
    }

    fn parse_brace_expr(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::LBrace)?;

        // Empty block
        if self.check(TokenKind::RBrace) {
            let end = self.advance().span.end;
            return Ok(CstExpr {
                kind: CstExprKind::Block {
                    stmts: Vec::new(),
                    result: None,
                },
                span: Span::new(start, end),
            });
        }

        // Check if this is a record literal (starts with ident:) or
        // a handler literal (starts with ident()
        if self.check(TokenKind::Ident) {
            let saved = self.pos;
            self.skip_trivia();
            let _ident = self.advance();
            self.skip_trivia();

            if self.check(TokenKind::Colon) {
                // It's a record
                self.pos = saved;
                return self.parse_record_literal(start);
            }

            if self.check(TokenKind::LParen) {
                // It's a handler literal: { method(params) => body, ... }
                self.pos = saved;
                return self.parse_handler_literal(start);
            }

            // It's a block, restore position
            self.pos = saved;
        }

        // Parse as block
        self.parse_block_contents(start)
    }

    fn parse_record_literal(&mut self, start: u32) -> Result<CstExpr, ParseError> {
        let mut fields = Vec::new();

        loop {
            if self.check(TokenKind::RBrace) {
                break;
            }

            let name = self.parse_ident()?;
            self.expect(TokenKind::Colon)?;
            let value = self.parse_expression()?;

            fields.push((name, value));

            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }

        let end = self.expect(TokenKind::RBrace)?.span.end;
        Ok(CstExpr {
            kind: CstExprKind::Record(fields),
            span: Span::new(start, end),
        })
    }

    /// Parse a handler literal: `{ method(params) => body, ... }`
    fn parse_handler_literal(&mut self, start: u32) -> Result<CstExpr, ParseError> {
        let mut methods = Vec::new();

        loop {
            if self.check(TokenKind::RBrace) {
                break;
            }

            let method_start = self.current().span.start;
            let method = self.parse_ident()?;

            // Parse parameters
            self.expect(TokenKind::LParen)?;
            let params = self.parse_params()?;
            self.expect(TokenKind::RParen)?;

            // Parse =>
            self.expect(TokenKind::FatArrow)?;

            // Parse body
            let body = self.parse_expression()?;
            let method_end = body.span.end;

            methods.push(CstHandlerLiteralMethod {
                method,
                params,
                body,
                span: Span::new(method_start, method_end),
            });

            // Optional comma between methods
            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }

        let end = self.expect(TokenKind::RBrace)?.span.end;
        Ok(CstExpr {
            kind: CstExprKind::HandlerLiteral(Box::new(CstHandlerLiteralExpr {
                methods,
                span: Span::new(start, end),
            })),
            span: Span::new(start, end),
        })
    }

    fn parse_block_expr(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::LBrace)?;
        self.parse_block_contents(start)
    }

    fn parse_block_contents(&mut self, start: u32) -> Result<CstExpr, ParseError> {
        let mut stmts = Vec::new();
        let mut result = None;

        while !self.check(TokenKind::RBrace) && !self.at_end() {
            let leading_trivia = self.skip_trivia();
            let stmt_start = self.current().span.start;

            // Let statement
            if self.check(TokenKind::Let) {
                self.advance();
                let name = self.parse_ident()?;

                let ty = if self.consume(TokenKind::Colon).is_some() {
                    Some(self.parse_type()?)
                } else {
                    None
                };

                self.expect(TokenKind::Eq)?;
                let init = self.parse_expression()?;
                let end = self.expect(TokenKind::Semi)?.span.end;

                stmts.push(CstStmt {
                    leading_trivia,
                    kind: CstStmtKind::Let(CstLetBinding { name, ty, init }),
                    span: Span::new(stmt_start, end),
                });
            } else {
                // Expression (statement or result)
                let expr = self.parse_expression()?;

                // Check if this is a statement (has semicolon) or result
                if self.consume(TokenKind::Semi).is_some() {
                    let end = self.current().span.start;
                    stmts.push(CstStmt {
                        leading_trivia,
                        kind: CstStmtKind::Expr(expr),
                        span: Span::new(stmt_start, end),
                    });
                } else {
                    // Final expression (result)
                    result = Some(Box::new(expr));
                    break;
                }
            }
        }

        let end = self.expect(TokenKind::RBrace)?.span.end;
        Ok(CstExpr {
            kind: CstExprKind::Block { stmts, result },
            span: Span::new(start, end),
        })
    }

    fn parse_if_expr(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::If)?;

        let condition = self.parse_expression()?;
        let then_branch = self.parse_block_expr()?;

        let else_branch = if self.consume(TokenKind::Else).is_some() {
            if self.check(TokenKind::If) {
                // else if
                Some(Box::new(self.parse_if_expr()?))
            } else {
                Some(Box::new(self.parse_block_expr()?))
            }
        } else {
            None
        };

        let end = else_branch
            .as_ref()
            .map_or(then_branch.span.end, |e| e.span.end);

        Ok(CstExpr {
            kind: CstExprKind::If {
                condition: Box::new(condition),
                then_branch: Box::new(then_branch),
                else_branch,
            },
            span: Span::new(start, end),
        })
    }

    fn parse_match_expr(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::Match)?;

        let scrutinee = self.parse_expression()?;
        self.expect(TokenKind::LBrace)?;

        let mut arms = Vec::new();
        loop {
            if self.check(TokenKind::RBrace) {
                break;
            }

            arms.push(self.parse_match_arm()?);

            // Optional comma between arms
            self.consume(TokenKind::Comma);
        }

        let end = self.expect(TokenKind::RBrace)?.span.end;

        Ok(CstExpr {
            kind: CstExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            span: Span::new(start, end),
        })
    }

    fn parse_match_arm(&mut self) -> Result<CstMatchArm, ParseError> {
        let start = self.current().span.start;
        let pattern = self.parse_pattern()?;

        let guard = if self.consume(TokenKind::If).is_some() {
            Some(self.parse_expression()?)
        } else {
            None
        };

        self.expect(TokenKind::FatArrow)?;
        let body = self.parse_expression()?;
        let end = body.span.end;

        Ok(CstMatchArm {
            pattern,
            guard,
            body,
            span: Span::new(start, end),
        })
    }

    fn parse_handle_expr(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::Handle)?;

        let body = self.parse_expression()?;

        // Parse optional `with` clause for handler values
        let mut handler_values = Vec::new();
        if self.consume(TokenKind::With).is_some() {
            // Parse comma-separated handler expressions until we see `{`
            loop {
                let handler_expr = self.parse_primary_expr()?;
                handler_values.push(handler_expr);

                if self.consume(TokenKind::Comma).is_none() {
                    break;
                }
            }
        }

        self.expect(TokenKind::LBrace)?;

        let mut handlers = Vec::new();
        let mut else_clause = None;

        loop {
            if self.check(TokenKind::RBrace) {
                break;
            }

            // Check for else clause
            if self.consume(TokenKind::Else).is_some() {
                self.expect(TokenKind::LBrace)?;
                // The else clause binds the result value
                // Syntax: else { (result) => expr }
                let else_body = self.parse_expression()?;
                self.expect(TokenKind::RBrace)?;
                else_clause = Some(else_body);
                break;
            }

            // Parse handler: Ability.method(params) => body
            // parse_qualified_name consumes all segments, so we need to split off the method
            let mut full_name = self.parse_qualified_name()?;

            // The last segment is the method name
            if full_name.segments.len() < 2 {
                return Err(ParseError::new(
                    ParseErrorKind::InvalidAbilitySyntax(
                        "handler must specify Ability.method".into(),
                    ),
                    full_name.span,
                ));
            }

            let method = full_name.segments.pop().expect("checked length above");
            let ability = CstQualifiedName {
                span: if full_name.segments.len() == 1 {
                    full_name.segments[0].span
                } else {
                    Span::new(
                        full_name.segments[0].span.start,
                        full_name.segments.last().expect("segments not empty").span.end,
                    )
                },
                segments: full_name.segments,
            };

            self.expect(TokenKind::LParen)?;
            let params = self.parse_params()?;
            self.expect(TokenKind::RParen)?;
            self.expect(TokenKind::FatArrow)?;

            let handler_body = if self.check(TokenKind::LBrace) {
                self.parse_block_expr()?
            } else {
                self.parse_expression()?
            };

            let end = handler_body.span.end;

            handlers.push(CstHandler {
                ability,
                method,
                params,
                body: handler_body,
                span: Span::new(start, end),
            });

            // Optional comma/newline between handlers
            self.consume(TokenKind::Comma);
        }

        let end = self.expect(TokenKind::RBrace)?.span.end;

        Ok(CstExpr {
            kind: CstExprKind::Handle(Box::new(CstHandleExpr {
                body,
                handler_values,
                handlers,
                else_clause,
                span: Span::new(start, end),
            })),
            span: Span::new(start, end),
        })
    }

    fn parse_resume_expr(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::Resume)?;
        self.expect(TokenKind::LParen)?;
        let value = self.parse_expression()?;
        let end = self.expect(TokenKind::RParen)?.span.end;

        Ok(CstExpr {
            kind: CstExprKind::Resume(Box::new(value)),
            span: Span::new(start, end),
        })
    }

    /// Parse a sandbox expression: `sandbox with Ability { body }` or `sandbox { body }`
    fn parse_sandbox_expr(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::Sandbox)?;

        // Parse optional `with` clause for allowed abilities
        let allowed_abilities = if self.consume(TokenKind::With).is_some() {
            self.parse_ability_list()?
        } else {
            Vec::new()
        };

        // Parse the body block
        let body = self.parse_block_expr()?;
        let end = body.span.end;

        Ok(CstExpr {
            kind: CstExprKind::Sandbox(Box::new(CstSandboxExpr {
                allowed_abilities,
                body,
                span: Span::new(start, end),
            })),
            span: Span::new(start, end),
        })
    }

    fn parse_args(&mut self) -> Result<Vec<CstExpr>, ParseError> {
        let mut args = Vec::new();

        loop {
            if self.check(TokenKind::RParen) {
                break;
            }

            args.push(self.parse_expression()?);

            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }

        Ok(args)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Pattern parsing
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_pattern(&mut self) -> Result<CstPattern, ParseError> {
        let start = self.current().span.start;

        // Wildcard
        if self.consume(TokenKind::Underscore).is_some() {
            return Ok(CstPattern {
                kind: CstPatternKind::Wildcard,
                span: Span::new(start, self.current().span.start),
            });
        }

        // Literal patterns
        if self.check(TokenKind::True) {
            self.advance();
            return Ok(CstPattern {
                kind: CstPatternKind::Literal(CstLiteral::Bool(true)),
                span: Span::new(start, self.current().span.start),
            });
        }

        if self.check(TokenKind::False) {
            self.advance();
            return Ok(CstPattern {
                kind: CstPatternKind::Literal(CstLiteral::Bool(false)),
                span: Span::new(start, self.current().span.start),
            });
        }

        if self.check(TokenKind::Number) {
            let token = self.advance();
            let value: f64 = token
                .text
                .parse()
                .map_err(|_| ParseError::new(ParseErrorKind::InvalidPattern, token.span))?;
            return Ok(CstPattern {
                kind: CstPatternKind::Literal(CstLiteral::Number(value)),
                span: token.span,
            });
        }

        if self.check(TokenKind::String) {
            let token = self.advance();
            let value = self.unescape_string(&token.text)?;
            return Ok(CstPattern {
                kind: CstPatternKind::Literal(CstLiteral::String(value.into())),
                span: token.span,
            });
        }

        // Tuple pattern
        if self.check(TokenKind::LParen) {
            self.advance();

            // Unit pattern
            if self.check(TokenKind::RParen) {
                let end = self.advance().span.end;
                return Ok(CstPattern {
                    kind: CstPatternKind::Literal(CstLiteral::Unit),
                    span: Span::new(start, end),
                });
            }

            let mut elements = Vec::new();
            loop {
                if self.check(TokenKind::RParen) {
                    break;
                }

                elements.push(self.parse_pattern()?);

                if self.consume(TokenKind::Comma).is_none() {
                    break;
                }
            }

            let end = self.expect(TokenKind::RParen)?.span.end;
            return Ok(CstPattern {
                kind: CstPatternKind::Tuple(elements),
                span: Span::new(start, end),
            });
        }

        // Record pattern
        if self.check(TokenKind::LBrace) {
            self.advance();

            let mut fields = Vec::new();
            loop {
                if self.check(TokenKind::RBrace) {
                    break;
                }

                let field_start = self.current().span.start;
                let field = self.parse_ident()?;

                let pattern = if self.consume(TokenKind::Colon).is_some() {
                    Some(self.parse_pattern()?)
                } else {
                    None
                };

                let field_end = pattern.as_ref().map_or(field.span.end, |p| p.span.end);

                fields.push(CstRecordPatternField {
                    field,
                    pattern,
                    span: Span::new(field_start, field_end),
                });

                if self.consume(TokenKind::Comma).is_none() {
                    break;
                }
            }

            let end = self.expect(TokenKind::RBrace)?.span.end;
            return Ok(CstPattern {
                kind: CstPatternKind::Record(fields),
                span: Span::new(start, end),
            });
        }

        // Identifier (binding or variant)
        if self.check(TokenKind::Ident) {
            let ident = self.parse_ident()?;

            // Check for variant pattern with payload
            if self.check(TokenKind::LParen) {
                self.advance();
                let payload = self.parse_pattern()?;
                let end = self.expect(TokenKind::RParen)?.span.end;

                return Ok(CstPattern {
                    kind: CstPatternKind::Variant {
                        name: CstQualifiedName {
                            segments: vec![ident],
                            span: Span::new(start, end),
                        },
                        payload: Some(Box::new(payload)),
                    },
                    span: Span::new(start, end),
                });
            }

            // Check for qualified variant
            if self.check(TokenKind::Dot) {
                let mut segments = vec![ident];
                while self.consume(TokenKind::Dot).is_some() {
                    segments.push(self.parse_ident()?);
                }

                let payload = if self.check(TokenKind::LParen) {
                    self.advance();
                    let p = self.parse_pattern()?;
                    self.expect(TokenKind::RParen)?;
                    Some(Box::new(p))
                } else {
                    None
                };

                let end = payload.as_ref().map_or(segments.last().expect("segments not empty").span.end, |p| p.span.end);

                return Ok(CstPattern {
                    kind: CstPatternKind::Variant {
                        name: CstQualifiedName {
                            segments,
                            span: Span::new(start, end),
                        },
                        payload,
                    },
                    span: Span::new(start, end),
                });
            }

            // Simple binding
            let span = ident.span;
            return Ok(CstPattern {
                kind: CstPatternKind::Binding(ident),
                span,
            });
        }

        Err(ParseError::new(ParseErrorKind::InvalidPattern, self.current().span))
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

    fn unescape_string(&self, text: &str) -> Result<String, ParseError> {
        // Remove quotes
        let content = text.trim_start_matches('"').trim_end_matches('"');
        self.unescape_string_part(content)
    }

    fn unescape_string_part(&self, text: &str) -> Result<String, ParseError> {
        let mut result = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => result.push('\n'),
                    Some('r') => result.push('\r'),
                    Some('t') => result.push('\t'),
                    Some('\\') => result.push('\\'),
                    Some('"') => result.push('"'),
                    Some('$') => result.push('$'),
                    Some(other) => {
                        // This shouldn't happen if lexer is correct
                        result.push('\\');
                        result.push(other);
                    }
                    None => result.push('\\'),
                }
            } else {
                result.push(c);
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

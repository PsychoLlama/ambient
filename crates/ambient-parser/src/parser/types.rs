//! Type expression parsing.

use ambient_engine::ast::Span;

use super::Parser;
use crate::cst::{CstQualifiedName, CstTypeExpr, CstTypeExprKind};
use crate::error::ParseError;
use crate::lexer::TokenKind;

impl Parser<'_> {
    /// Parse a type expression.
    ///
    /// # Errors
    ///
    /// Returns a `ParseError` if the source is not a valid type expression.
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

        // `Handler<A>` / `Handler<A, R>` is type syntax, not a nominal name:
        // `A` is an ability reference, not a type. Recognize it here (like
        // function arrows and tuples) so it never flows through the generic /
        // name path as an identifier.
        if base.segments.len() == 1
            && base.segments[0].name.as_ref() == "Handler"
            && self.check(TokenKind::Lt)
        {
            return self.parse_handler_type(start);
        }

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

    /// Parse the arguments of a `Handler<A>` / `Handler<A, R>` type. The
    /// leading `Handler` name has been consumed; `self` is positioned at the
    /// opening `<`. `A` is an ability reference (a qualified name), `R` an
    /// optional answer type.
    fn parse_handler_type(&mut self, start: u32) -> Result<CstTypeExpr, ParseError> {
        self.expect(TokenKind::Lt)?;
        let ability = self.parse_qualified_name()?;
        let answer = if self.consume(TokenKind::Comma).is_some() {
            Some(Box::new(self.parse_type()?))
        } else {
            None
        };
        let end = self.expect(TokenKind::Gt)?.span.end;
        Ok(CstTypeExpr {
            kind: CstTypeExprKind::Handler { ability, answer },
            span: Span::new(start, end),
        })
    }

    pub(super) fn parse_tuple_or_function_type(&mut self) -> Result<CstTypeExpr, ParseError> {
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

    pub(super) fn parse_record_type(&mut self) -> Result<CstTypeExpr, ParseError> {
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

    pub(super) fn parse_ability_list(&mut self) -> Result<Vec<CstQualifiedName>, ParseError> {
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
}

//! Pattern parsing.

use ambient_engine::ast::Span;

use super::Parser;
use crate::cst::{CstLiteral, CstPattern, CstPatternKind, CstQualifiedName, CstRecordPatternField};
use crate::error::{ParseError, ParseErrorKind};
use crate::lexer::TokenKind;

impl Parser<'_> {
    #[allow(clippy::too_many_lines)]
    pub(super) fn parse_pattern(&mut self) -> Result<CstPattern, ParseError> {
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
            let value = Self::unescape_string(&token.text);
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

                let end = payload
                    .as_ref()
                    .map_or(segments.last().expect("segments not empty").span.end, |p| {
                        p.span.end
                    });

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

        Err(ParseError::new(
            ParseErrorKind::InvalidPattern,
            self.current().span,
        ))
    }
}

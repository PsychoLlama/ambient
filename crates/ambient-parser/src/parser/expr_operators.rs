//! Binary and unary operator parsing: the precedence-climbing ladder from
//! `||` down to unary `-`/`!`, split out of `expr.rs`. Primary/postfix
//! expression parsing stays there.

use ambient_engine::ast::Span;

use super::Parser;
use crate::cst::{CstBinaryOp, CstExpr, CstExprKind, CstUnaryOp};
use crate::error::ParseError;
use crate::lexer::TokenKind;

impl Parser<'_> {
    pub(super) fn parse_or_expr(&mut self) -> Result<CstExpr, ParseError> {
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
}

//! Block and control-flow expression parsing: blocks, `if`, `match`,
//! `with … handle`, `resume`, and `sandbox`.

use ambient_engine::ast::Span;

use super::Parser;
use crate::cst::{
    CstExpr, CstExprKind, CstIdent, CstLetBinding, CstMatchArm, CstQualifiedName, CstSandboxExpr,
    CstStmt, CstStmtKind, CstWithHandleExpr,
};
use crate::error::{ParseError, ParseErrorKind};
use crate::lexer::TokenKind;

impl Parser<'_> {
    pub(super) fn parse_block_expr(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::LBrace)?;
        self.parse_block_contents(start)
    }

    pub(super) fn parse_block_contents(&mut self, start: u32) -> Result<CstExpr, ParseError> {
        let mut stmts = Vec::new();
        let mut result = None;

        while !self.check(TokenKind::RBrace) && !self.at_end() {
            let leading_trivia = self.skip_trivia();
            let stmt_start = self.current().span.start;

            // Block-scoped import
            if self.check(TokenKind::Use) {
                let use_def = self.parse_use(false)?;
                let end = self.current().span.start;
                stmts.push(CstStmt {
                    leading_trivia,
                    kind: CstStmtKind::Use(use_def),
                    span: Span::new(stmt_start, end),
                });
                continue;
            }

            // Block-scoped constant. Shares `parse_const` with module-level
            // items, so the grammar is identical (`const NAME[: Type] = …;`).
            // A block `const` is never `pub`.
            if self.check(TokenKind::Const) {
                let const_def = self.parse_const(false)?;
                let end = self.current().span.start;
                stmts.push(CstStmt {
                    leading_trivia,
                    kind: CstStmtKind::Const(const_def),
                    span: Span::new(stmt_start, end),
                });
                continue;
            }

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

                // Check if this is a statement (has semicolon) or result.
                // Block-bodied expressions (`if`, `match`, `{ … }`,
                // `with … handle`) may also stand alone as statements
                // without a semicolon when more code follows, Rust-style;
                // in final position they remain the block's result.
                let block_bodied = matches!(
                    expr.kind,
                    CstExprKind::If { .. }
                        | CstExprKind::Match { .. }
                        | CstExprKind::Block { .. }
                        | CstExprKind::Handle(_)
                );
                if self.consume(TokenKind::Semi).is_some()
                    || (block_bodied && !self.check(TokenKind::RBrace) && !self.at_end())
                {
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

    pub(super) fn parse_if_expr(&mut self) -> Result<CstExpr, ParseError> {
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

    pub(super) fn parse_match_expr(&mut self) -> Result<CstExpr, ParseError> {
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

    /// Parse a handle expression: `with H₁, …, Hₙ handle BODY [else E]`.
    ///
    /// Each `Hᵢ` is an ordinary expression (a handler literal or a
    /// `Handler<A, R>` value). The `handle` keyword terminates the list: it
    /// cannot continue an expression, so the comma-separated list ends
    /// deterministically.
    pub(super) fn parse_with_handle_expr(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::With)?;

        // Comma-separated handler expressions until `handle`.
        let mut handlers = Vec::new();
        loop {
            let handler = self.parse_expression()?;
            handlers.push(handler);
            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }

        self.expect(TokenKind::Handle)?;
        let body = self.parse_expression()?;

        // Optional `else EXPR` transform, applied to the body's result on
        // normal completion.
        let else_clause = if self.consume(TokenKind::Else).is_some() {
            Some(self.parse_expression()?)
        } else {
            None
        };

        let end = else_clause.as_ref().map_or(body.span.end, |e| e.span.end);

        Ok(CstExpr {
            kind: CstExprKind::Handle(Box::new(CstWithHandleExpr {
                handlers,
                body,
                else_clause,
                span: Span::new(start, end),
            })),
            span: Span::new(start, end),
        })
    }

    /// Parse a handler-arm head `Ability::method`, returning the ability's
    /// qualified name and the trailing method segment.
    pub(super) fn parse_handler_arm_head(
        &mut self,
    ) -> Result<(CstQualifiedName, CstIdent), ParseError> {
        let mut full_name = self.parse_qualified_name()?;

        if full_name.segments.len() < 2 {
            return Err(ParseError::new(
                ParseErrorKind::InvalidAbilitySyntax(
                    "handler arm must specify Ability::method".into(),
                ),
                full_name.span,
            ));
        }

        let method = full_name.segments.pop().expect("checked length above");
        let ability = CstQualifiedName {
            span: Span::new(
                full_name.segments[0].span.start,
                full_name
                    .segments
                    .last()
                    .expect("segments not empty")
                    .span
                    .end,
            ),
            segments: full_name.segments,
        };
        Ok((ability, method))
    }

    pub(super) fn parse_resume_expr(&mut self) -> Result<CstExpr, ParseError> {
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
    pub(super) fn parse_sandbox_expr(&mut self) -> Result<CstExpr, ParseError> {
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
}

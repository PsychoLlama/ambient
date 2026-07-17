//! Expression parsing: primary and postfix expressions. The binary/unary
//! operator precedence ladder lives in `expr_operators.rs`.

use ambient_engine::ast::Span;

use super::Parser;
use crate::cst::{
    CstExpr, CstExprKind, CstHandlerLiteralExpr, CstHandlerLiteralMethod, CstIdent, CstLambda,
    CstQualifiedName,
};
use crate::error::{ParseError, ParseErrorKind};
use crate::lexer::TokenKind;

impl Parser<'_> {
    /// Parse an expression.
    ///
    /// # Errors
    ///
    /// Returns a `ParseError` if the source is not a valid expression.
    pub fn parse_expression(&mut self) -> Result<CstExpr, ParseError> {
        self.parse_or_expr()
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn parse_postfix_expr(&mut self) -> Result<CstExpr, ParseError> {
        let mut expr = self.parse_primary_expr()?;

        loop {
            if self.consume(TokenKind::Dot).is_some() {
                // Field access or tuple index
                if self.check(TokenKind::Number) {
                    let token = self.advance();
                    let index: u32 = token.text.parse().map_err(|_| {
                        ParseError::new(ParseErrorKind::InvalidExpression, token.span)
                    })?;
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

                    // Check for ability method call (requires ! sigil)
                    if self.check(TokenKind::Bang) {
                        self.advance();
                        self.expect(TokenKind::LParen)?;
                        let args = self.parse_args()?;
                        let end = self.expect(TokenKind::RParen)?.span.end;

                        // Reconstruct ability name from the expression
                        // Handles: Ident, QualifiedName, and chains of Field accesses
                        let ability = expr_to_qualified_name(&expr).ok_or_else(|| {
                            ParseError::new(
                                ParseErrorKind::InvalidAbilitySyntax(
                                    "expected ability name before method".into(),
                                ),
                                expr.span,
                            )
                        })?;

                        let span = Span::new(expr.span.start, end);
                        expr = CstExpr {
                            kind: CstExprKind::Perform {
                                ability,
                                method: field,
                                args,
                            },
                            span,
                        };
                    } else if self.check(TokenKind::LParen) {
                        // Method call: receiver.method(args)
                        self.advance();
                        let args = self.parse_args()?;
                        let end = self.expect(TokenKind::RParen)?.span.end;
                        let span = Span::new(expr.span.start, end);
                        expr = CstExpr {
                            kind: CstExprKind::MethodCall {
                                receiver: Box::new(expr),
                                method: field,
                                args,
                            },
                            span,
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
                    // Bare-method perform: `seed!(args)`. The ability is
                    // unspelled (empty segments); it is implied by an
                    // imported ability method (`use m::Random::seed;`) and
                    // filled in by the resolve pass.
                    CstExprKind::Ident(ident) => {
                        let method = ident.clone();
                        self.expect(TokenKind::LParen)?;
                        let args = self.parse_args()?;
                        let end = self.expect(TokenKind::RParen)?.span.end;

                        expr = CstExpr {
                            kind: CstExprKind::Perform {
                                ability: CstQualifiedName {
                                    segments: vec![],
                                    span: method.span,
                                },
                                method,
                                args,
                            },
                            span: Span::new(expr.span.start, end),
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

    #[allow(clippy::too_many_lines)]
    pub(super) fn parse_primary_expr(&mut self) -> Result<CstExpr, ParseError> {
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
            let value: f64 = token.text.parse().map_err(|_| {
                ParseError::new(
                    ParseErrorKind::InvalidNumber(token.text.clone()),
                    token.span,
                )
            })?;
            return Ok(CstExpr {
                kind: CstExprKind::Number(value),
                span: token.span,
            });
        }

        if self.check(TokenKind::String) {
            let token = self.advance();
            let value = Self::unescape_string(&token.text);
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

        // Handle expression: `with H₁, …, Hₙ handle BODY [else E]`.
        // `with` only ever appears postfix elsewhere (ability clauses,
        // sandbox), so a `with`-led prefix is unambiguous here.
        if self.check(TokenKind::With) {
            return self.parse_with_handle_expr();
        }

        // Resume expression: resume(value)
        if self.check(TokenKind::Resume) {
            return self.parse_resume_expr();
        }

        // Return expression: `return expr` or bare `return`
        if self.check(TokenKind::Return) {
            return self.parse_return_expr();
        }

        // Sandbox expression: sandbox with Ability { ... } or sandbox { ... }
        if self.check(TokenKind::Sandbox) {
            return self.parse_sandbox_expr();
        }

        // Identifier or qualified name (including pkg::module::function, core::module::function,
        // and workspace-rooted ::other_package::function)
        // Note: Self_ is NOT included here because in expressions `self` is an identifier
        // (the instance in a method), not a module prefix. Module prefix `self` is only
        // valid in import statements, which are parsed separately.
        if self.check(TokenKind::Ident)
            || self.check(TokenKind::ColonColon)
            || matches!(
                self.current_kind(),
                TokenKind::Pkg | TokenKind::Core | TokenKind::Super | TokenKind::Self_
            )
        {
            return self.parse_ident_or_qualified();
        }

        Err(ParseError::new(
            ParseErrorKind::UnexpectedToken(format!("{:?}", self.current_kind())),
            self.current().span,
        ))
    }

    fn parse_ident_or_qualified(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;

        // A leading `::` roots the path at the workspace (`::other_pkg::item`),
        // recorded as an empty head segment spanning the separator — the same
        // spelling the AST's `QualifiedName` carries. The next segment names a
        // package, so it must be a plain identifier, never a prefix keyword.
        let workspace_root = self.parse_workspace_root();

        // Parse the head segment. Module-prefix keywords (`pkg`, `core`,
        // `super`) and `self` are lexed as their own token kinds rather than as
        // `Ident`, so accept them explicitly as the head of a path. In an
        // expression `self` is normally the instance reference, but it may also
        // head a qualified name, so it is handled uniformly here.
        let ident = if workspace_root.is_some() {
            self.parse_ident()?
        } else {
            self.parse_path_segment()?
        };

        // A `::` after the head starts a qualified name (`core::primitives::number::abs`,
        // `core::system::FileSystem`, `stats::mean`). A plain `.` is left to postfix
        // parsing, where it means field access, method call, or tuple index.
        if workspace_root.is_some() || self.check(TokenKind::ColonColon) {
            let mut segments: Vec<CstIdent> = workspace_root.into_iter().collect();
            segments.push(ident);
            while self.consume(TokenKind::ColonColon).is_some() {
                if !self.check(TokenKind::Ident) {
                    break;
                }
                segments.push(self.parse_ident()?);
            }

            if segments.len() > 1 {
                // Ability method call pattern: `Ability::method!(args)`
                if self.check(TokenKind::Bang) {
                    return self.parse_ability_call(segments);
                }

                let span = Span::new(
                    segments[0].span.start,
                    segments.last().expect("segments not empty").span.end,
                );
                let qualified_name = CstQualifiedName { segments, span };

                // Typed record construction: `Qualified::Name { field: value }`
                if self.check(TokenKind::LBrace) && self.is_record_literal_start() {
                    return self.parse_typed_record_literal(qualified_name, start);
                }

                return Ok(CstExpr {
                    kind: CstExprKind::QualifiedName(qualified_name),
                    span,
                });
            }

            // A trailing `::` with no following identifier: fall through and
            // treat the head as a bare identifier.
            let single_ident = segments.into_iter().next().expect("segments not empty");
            let span = single_ident.span;
            return Ok(CstExpr {
                kind: CstExprKind::Ident(single_ident),
                span,
            });
        }

        // No `::` — a bare identifier. It may still introduce a typed record
        // literal: `TypeName { field: value, ... }`.
        if self.check(TokenKind::LBrace) && self.is_record_literal_start() {
            let ident_span = ident.span;
            let qualified_name = CstQualifiedName {
                segments: vec![ident],
                span: ident_span,
            };
            return self.parse_typed_record_literal(qualified_name, start);
        }

        let span = ident.span;
        Ok(CstExpr {
            kind: CstExprKind::Ident(ident),
            span,
        })
    }

    /// Parse a single path-segment head: a regular identifier, or a
    /// module-prefix keyword (`pkg`, `core`, `super`, `self`) lexed as its own
    /// token kind. Shared with pattern parsing — do not fork a second reader.
    pub(super) fn parse_path_segment(&mut self) -> Result<CstIdent, ParseError> {
        match self.current_kind() {
            TokenKind::Pkg | TokenKind::Core | TokenKind::Super | TokenKind::Self_ => {
                let token = self.advance();
                let trailing_trivia = self.skip_trivia();
                Ok(CstIdent {
                    name: token.text.into(),
                    span: token.span,
                    trailing_trivia,
                })
            }
            _ => self.parse_ident(),
        }
    }

    /// Parse an ability call pattern: Ability.method!(args)
    fn parse_ability_call(&mut self, mut segments: Vec<CstIdent>) -> Result<CstExpr, ParseError> {
        self.expect(TokenKind::Bang)?;
        self.expect(TokenKind::LParen)?;
        let args = self.parse_args()?;
        let end = self.expect(TokenKind::RParen)?.span.end;

        // Last segment is the method, everything else is the ability
        let method = segments.pop().expect("at least 2 segments");
        let ability_span = Span::new(
            segments[0].span.start,
            segments.last().expect("at least 1 segment").span.end,
        );
        let ability = CstQualifiedName {
            segments,
            span: ability_span,
        };
        let span = Span::new(ability_span.start, end);

        Ok(CstExpr {
            kind: CstExprKind::Perform {
                ability,
                method,
                args,
            },
            span,
        })
    }

    fn parse_paren_expr(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::LParen)?;

        // Empty parens: unit literal, or a zero-parameter lambda `() => body`.
        if self.check(TokenKind::RParen) {
            let saved = self.pos;
            self.advance();
            self.skip_trivia();
            if self.check(TokenKind::FatArrow) {
                // Rewind to the `)` — parse_lambda parses (empty) params
                // and expects to consume the closing paren itself.
                self.pos = saved;
                return self.parse_lambda(start);
            }
            self.pos = saved;
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
        // The caller already consumed the outer `(`, so start one level deep
        // and depth-match every token from here: a blind advance past the
        // first drops its bump when it is a `(` (`(() => 2, 40)`), misreading.
        let mut depth = 1;
        let saved = self.pos;

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

        // A handler literal is `{ Ability::method( … ) => …, … }`: a
        // qualified name leading to a parameter list and a `=>`. This is
        // distinguished from a block whose first statement is a qualified
        // call (`{ Stdio::foo(x) }`) by the trailing `=>`.
        if self.is_handler_literal_start() {
            return self.parse_handler_literal(start);
        }

        // Record literal: `{ ident: … }`.
        if self.check(TokenKind::Ident) {
            let saved = self.pos;
            self.skip_trivia();
            let _ident = self.advance();
            self.skip_trivia();

            if self.check(TokenKind::Colon) {
                self.pos = saved;
                return self.parse_record_literal(start);
            }

            // It's a block, restore position
            self.pos = saved;
        }

        // Parse as block
        self.parse_block_contents(start)
    }

    /// Lookahead: does the position after `{` begin a handler-literal arm
    /// (`Ability::method( … ) =>`)? Restores the cursor before returning.
    fn is_handler_literal_start(&mut self) -> bool {
        let saved = self.pos;
        let result = self.scan_handler_literal_arm();
        self.pos = saved;
        result
    }

    /// Scan (destructively) one `Ability::method( … ) =>` arm head, returning
    /// whether it matched. Callers restore the cursor.
    fn scan_handler_literal_arm(&mut self) -> bool {
        // Head segment: an identifier or a module-prefix keyword.
        self.skip_trivia();
        if !matches!(
            self.current_kind(),
            TokenKind::Ident
                | TokenKind::Core
                | TokenKind::Pkg
                | TokenKind::Super
                | TokenKind::Self_
        ) {
            return false;
        }
        self.advance();

        // Require at least one `::segment` (arms are always qualified).
        let mut qualified = false;
        while self.check(TokenKind::ColonColon) {
            self.advance();
            if !self.check(TokenKind::Ident) {
                return false;
            }
            self.advance();
            qualified = true;
        }
        if !qualified || !self.check(TokenKind::LParen) {
            return false;
        }

        // Skip the balanced parameter list, then look for `=>`.
        let mut depth = 0usize;
        loop {
            self.skip_trivia();
            match self.current_kind() {
                TokenKind::LParen => {
                    depth += 1;
                    self.pos += 1;
                }
                TokenKind::RParen => {
                    depth -= 1;
                    self.pos += 1;
                    if depth == 0 {
                        break;
                    }
                }
                TokenKind::Eof => return false,
                _ => self.pos += 1,
            }
        }

        self.check(TokenKind::FatArrow)
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

    /// Check if the upcoming `{` starts a record literal (has `ident:` pattern).
    /// Returns false for empty braces `{}` or block expressions.
    fn is_record_literal_start(&mut self) -> bool {
        let saved = self.pos;
        self.skip_trivia();
        self.pos += 1; // skip the `{` token
        self.skip_trivia();

        // Empty braces {} are ambiguous (could be empty block, empty record,
        // or syntax like `handle x with h {}`), so don't treat as typed record.
        // Typed record construction requires at least one field: `TypeName { field: value }`.
        if self.current_kind() == TokenKind::RBrace {
            self.pos = saved;
            return false;
        }

        // Check for ident followed by colon
        let is_record = if self.current_kind() == TokenKind::Ident {
            self.pos += 1; // skip ident
            self.skip_trivia();
            self.current_kind() == TokenKind::Colon
        } else {
            false
        };

        self.pos = saved;
        is_record
    }

    /// Parse a typed record literal: `TypeName { field: value, ... }`
    fn parse_typed_record_literal(
        &mut self,
        type_name: CstQualifiedName,
        start: u32,
    ) -> Result<CstExpr, ParseError> {
        self.expect(TokenKind::LBrace)?;

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
            kind: CstExprKind::TypedRecord { type_name, fields },
            span: Span::new(start, end),
        })
    }

    /// Parse a handler literal: `{ Ability::method(params) => body, ... }`
    fn parse_handler_literal(&mut self, start: u32) -> Result<CstExpr, ParseError> {
        let mut methods = Vec::new();

        loop {
            if self.check(TokenKind::RBrace) {
                break;
            }

            let method_start = self.current().span.start;

            // Parse the qualified `Ability::method` prefix, then split off
            // the final segment as the method name.
            let (ability, method) = self.parse_handler_arm_head()?;

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
                ability,
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

    pub(super) fn parse_args(&mut self) -> Result<Vec<CstExpr>, ParseError> {
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
}

/// Convert an expression to a qualified name if possible.
///
/// Handles:
/// - `Ident` → single-segment qualified name
/// - `QualifiedName` → as-is
/// - Chain of `Field` accesses → multi-segment qualified name (e.g., `platform.Stdio`)
///
/// Returns `None` if the expression cannot be converted to a qualified name.
fn expr_to_qualified_name(expr: &CstExpr) -> Option<CstQualifiedName> {
    match &expr.kind {
        CstExprKind::Ident(ident) => Some(CstQualifiedName {
            segments: vec![ident.clone()],
            span: expr.span,
        }),
        CstExprKind::QualifiedName(qn) => Some(qn.clone()),
        CstExprKind::Field { record, field } => {
            // Recursively convert the record to a qualified name and append the field
            let mut qn = expr_to_qualified_name(record)?;
            qn.segments.push(field.clone());
            qn.span = Span::new(qn.span.start, field.span.end);
            Some(qn)
        }
        _ => None,
    }
}

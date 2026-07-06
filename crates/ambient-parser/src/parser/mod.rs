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
    CstIdent, CstImplDef, CstImplMethod, CstItem, CstItemKind, CstModule, CstParam,
    CstQualifiedName, CstReplInput, CstTraitDef, CstTraitMethod, CstTraitParam, CstTraitParamKind,
    CstTypeAliasDef, CstTypeParam, CstUseDef, CstUseTree, CstUseTreeKind, CstWhereClause, Trivia,
    TriviaItem, TriviaKind,
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
                    TokenKind::Use => CstItemKind::Use(self.parse_use(true)?),
                    TokenKind::Const => CstItemKind::Const(self.parse_const(true)?),
                    TokenKind::Type => CstItemKind::TypeAlias(self.parse_type_alias(true)?),
                    TokenKind::Struct => CstItemKind::TypeAlias(self.parse_struct_def(true, None)?),
                    TokenKind::Enum => CstItemKind::Enum(self.parse_enum(true, None)?),
                    TokenKind::Unique => self.parse_unique_item(true)?,
                    TokenKind::Ability => CstItemKind::Ability(self.parse_ability_def(true)?),
                    TokenKind::Trait => CstItemKind::Trait(self.parse_trait_def(true)?),
                    _ => {
                        return Err(ParseError::new(
                            ParseErrorKind::Expected {
                                expected: "item declaration after pub".into(),
                                found: format!("{:?}", self.current_kind()),
                            },
                            self.current().span,
                        ));
                    }
                }
            }
            TokenKind::Fn => CstItemKind::Function(self.parse_function(false)?),
            TokenKind::Const => CstItemKind::Const(self.parse_const(false)?),
            TokenKind::Type => CstItemKind::TypeAlias(self.parse_type_alias(false)?),
            TokenKind::Struct => CstItemKind::TypeAlias(self.parse_struct_def(false, None)?),
            TokenKind::Enum => CstItemKind::Enum(self.parse_enum(false, None)?),
            TokenKind::Unique => self.parse_unique_item(false)?,
            TokenKind::Ability => CstItemKind::Ability(self.parse_ability_def(false)?),
            TokenKind::Use => CstItemKind::Use(self.parse_use(false)?),
            TokenKind::Trait => CstItemKind::Trait(self.parse_trait_def(false)?),
            TokenKind::Impl => CstItemKind::Impl(self.parse_impl_def()?),
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

    fn parse_const(&mut self, is_public: bool) -> Result<CstConstDef, ParseError> {
        self.expect(TokenKind::Const)?;
        let name = self.parse_ident()?;
        self.expect(TokenKind::Colon)?;
        let ty = self.parse_type()?;
        self.expect(TokenKind::Eq)?;
        let value = self.parse_expression()?;
        self.expect(TokenKind::Semi)?;

        Ok(CstConstDef {
            is_public,
            name,
            ty,
            value,
        })
    }

    /// Parse a `unique(<uuid>)` prefix, returning the UUID text.
    ///
    /// A canonical uppercase UUID lexes as a single `Uuid` token. If the
    /// parentheses hold anything else (lowercase, partial, or garbage), report
    /// a precise error but recover by skipping to the closing paren so the rest
    /// of the declaration — and editor completion while the UUID is still being
    /// typed — keeps working.
    fn parse_unique_prefix(&mut self) -> Result<Option<Arc<str>>, ParseError> {
        if self.consume(TokenKind::Unique).is_none() {
            return Ok(None);
        }
        self.expect(TokenKind::LParen)?;
        let id = if let Some(token) = self.consume(TokenKind::Uuid) {
            Some(Arc::from(token.text.as_str()))
        } else {
            let span = self.current().span;
            self.error(ParseError::new(ParseErrorKind::ExpectedUuid, span));
            while !self.check(TokenKind::RParen) && !self.at_end() {
                self.advance();
            }
            None
        };
        self.expect(TokenKind::RParen)?;
        Ok(id)
    }

    /// Parse a `unique(<uuid>)`-prefixed item: a nominal `struct` definition or
    /// a nominal `enum`. The `unique(...)` syntax is shared, so the prefix is
    /// parsed once here and the following keyword decides the item.
    fn parse_unique_item(&mut self, is_public: bool) -> Result<CstItemKind, ParseError> {
        let unique_id = self.parse_unique_prefix()?;
        self.skip_trivia();
        match self.current_kind() {
            TokenKind::Struct => Ok(CstItemKind::TypeAlias(
                self.parse_struct_def(is_public, unique_id)?,
            )),
            TokenKind::Enum => Ok(CstItemKind::Enum(self.parse_enum(is_public, unique_id)?)),
            other => Err(ParseError::new(
                ParseErrorKind::Expected {
                    expected: "`struct` or `enum` after `unique(...)`".into(),
                    found: format!("{other:?}"),
                },
                self.current().span,
            )),
        }
    }

    /// Parse a `struct Foo { fields }` record definition. Structs share the
    /// `CstTypeAliasDef` node (their body is a `Type::Record`), but require the
    /// record body — there is no `= Type` alias form.
    fn parse_struct_def(
        &mut self,
        is_public: bool,
        unique_id: Option<Arc<str>>,
    ) -> Result<CstTypeAliasDef, ParseError> {
        self.expect(TokenKind::Struct)?;
        let name = self.parse_ident()?;

        let type_params = if self.check(TokenKind::Lt) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };

        let ty = self.parse_record_type()?;

        Ok(CstTypeAliasDef {
            is_public,
            name,
            type_params,
            ty,
            unique_id,
        })
    }

    fn parse_type_alias(&mut self, is_public: bool) -> Result<CstTypeAliasDef, ParseError> {
        self.expect(TokenKind::Type)?;
        let name = self.parse_ident()?;

        let type_params = if self.check(TokenKind::Lt) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };

        // Aliases require `= Type`; the record form now uses `struct`.
        self.expect(TokenKind::Eq)?;
        let ty = self.parse_type()?;
        self.expect(TokenKind::Semi)?;

        Ok(CstTypeAliasDef {
            is_public,
            name,
            type_params,
            ty,
            unique_id: None,
        })
    }

    fn parse_enum(
        &mut self,
        is_public: bool,
        unique_id: Option<Arc<str>>,
    ) -> Result<CstEnumDef, ParseError> {
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
            is_public,
            name,
            type_params,
            variants,
            unique_id,
        })
    }

    fn parse_ability_def(&mut self, is_public: bool) -> Result<CstAbilityDef, ParseError> {
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
            is_public,
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

    // ─────────────────────────────────────────────────────────────────────────
    // Trait parsing
    // ─────────────────────────────────────────────────────────────────────────

    /// Parse a trait definition: `trait Name<T> with Supertrait { fn method(self, ...): RetType; }`
    fn parse_trait_def(&mut self, is_public: bool) -> Result<CstTraitDef, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::Trait)?;
        let name = self.parse_ident()?;

        // Type parameters
        let type_params = if self.check(TokenKind::Lt) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };

        // Supertraits: `with Trait1, Trait2`
        let supertraits = if self.check(TokenKind::With) {
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
            methods.push(self.parse_trait_method()?);
        }

        let end = self.expect(TokenKind::RBrace)?.span.end;

        Ok(CstTraitDef {
            is_public,
            name,
            type_params,
            supertraits,
            methods,
            span: Span::new(start, end),
        })
    }

    /// Parse a trait method signature: `fn name(self, args): RetType;`
    fn parse_trait_method(&mut self) -> Result<CstTraitMethod, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::Fn)?;
        let name = self.parse_ident()?;

        let type_params = if self.check(TokenKind::Lt) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };

        self.expect(TokenKind::LParen)?;
        let params = self.parse_trait_params()?;
        self.expect(TokenKind::RParen)?;

        self.expect(TokenKind::Colon)?;
        let ret_ty = self.parse_type()?;

        let end = self.expect(TokenKind::Semi)?.span.end;

        Ok(CstTraitMethod {
            name,
            type_params,
            params,
            ret_ty,
            span: Span::new(start, end),
        })
    }

    /// Parse trait method parameters (handles `self`).
    fn parse_trait_params(&mut self) -> Result<Vec<CstTraitParam>, ParseError> {
        let mut params = Vec::new();

        loop {
            if self.check(TokenKind::RParen) {
                break;
            }

            let start = self.current().span.start;

            // Check for `self` keyword
            if self.check(TokenKind::Self_) {
                let token = self.advance();
                params.push(CstTraitParam {
                    kind: CstTraitParamKind::SelfParam,
                    span: token.span,
                });
            } else {
                // Regular parameter: name: Type
                let name = self.parse_ident()?;
                self.expect(TokenKind::Colon)?;
                let ty = self.parse_type()?;
                let end = ty.span.end;
                params.push(CstTraitParam {
                    kind: CstTraitParamKind::Named { name, ty },
                    span: Span::new(start, end),
                });
            }

            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }

        Ok(params)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Impl parsing
    // ─────────────────────────────────────────────────────────────────────────

    /// Parse an impl block: `impl<T> Trait for Type where T: Bound { methods }`
    /// or an inherent impl: `impl<T> Type { methods }`.
    fn parse_impl_def(&mut self) -> Result<CstImplDef, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::Impl)?;

        // Optional type parameters
        let type_params = if self.check(TokenKind::Lt) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };

        // Disambiguate `impl Trait for Type` from inherent `impl Type`:
        // provisionally parse a qualified name; only if `for` follows was it
        // the trait name. Otherwise rewind and parse the target type (which
        // may be generic, e.g. `Option<T>` — not a qualified name).
        let snapshot = self.pos;
        let trait_name = match self.parse_qualified_name() {
            Ok(name) if self.check(TokenKind::For) => {
                self.advance();
                Some(name)
            }
            _ => {
                self.pos = snapshot;
                None
            }
        };

        // Type being implemented
        let for_type = self.parse_type()?;

        // Optional where clause
        let where_clauses = if self.check(TokenKind::Where) {
            self.advance();
            self.parse_where_clauses()?
        } else {
            Vec::new()
        };

        self.expect(TokenKind::LBrace)?;

        let mut methods = Vec::new();
        loop {
            if self.check(TokenKind::RBrace) {
                break;
            }
            methods.push(self.parse_impl_method()?);
        }

        let end = self.expect(TokenKind::RBrace)?.span.end;

        Ok(CstImplDef {
            type_params,
            trait_name,
            for_type,
            where_clauses,
            methods,
            span: Span::new(start, end),
        })
    }

    /// Parse where clauses: `T: Trait1 + Trait2, U: Trait3`
    fn parse_where_clauses(&mut self) -> Result<Vec<CstWhereClause>, ParseError> {
        let mut clauses = Vec::new();

        loop {
            let start = self.current().span.start;
            let ty = self.parse_type()?;
            self.expect(TokenKind::Colon)?;

            let mut bounds = Vec::new();
            loop {
                bounds.push(self.parse_qualified_name()?);
                if self.consume(TokenKind::Plus).is_none() {
                    break;
                }
            }

            let end = bounds.last().map_or(ty.span.end, |b| b.span.end);
            clauses.push(CstWhereClause {
                ty,
                bounds,
                span: Span::new(start, end),
            });

            if self.consume(TokenKind::Comma).is_none() {
                break;
            }

            // Don't continue if we hit the opening brace
            if self.check(TokenKind::LBrace) {
                break;
            }
        }

        Ok(clauses)
    }

    /// Parse an impl method: `fn name(self, args): RetType { body }`
    fn parse_impl_method(&mut self) -> Result<CstImplMethod, ParseError> {
        let start = self.current().span.start;
        self.expect(TokenKind::Fn)?;
        let name = self.parse_ident()?;

        let type_params = if self.check(TokenKind::Lt) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };

        self.expect(TokenKind::LParen)?;
        let params = self.parse_trait_params()?;
        self.expect(TokenKind::RParen)?;

        // Optional return type
        let ret_ty = if self.consume(TokenKind::Colon).is_some() {
            Some(self.parse_type()?)
        } else {
            None
        };

        // Optional ability clause
        let abilities = if self.check(TokenKind::With) {
            self.advance();
            self.parse_ability_list()?
        } else {
            Vec::new()
        };

        // Method body
        let body = self.parse_block_expr()?;
        let end = body.span.end;

        Ok(CstImplMethod {
            name,
            type_params,
            params,
            ret_ty,
            abilities,
            body,
            span: Span::new(start, end),
        })
    }

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
    /// `as` is contextual (an identifier in alias position), like
    /// `platform` in path-head position. Keyword segments are only valid
    /// at the head of a full path — validated during lowering, where the
    /// flattened paths exist.
    fn parse_use(&mut self, is_public: bool) -> Result<CstUseDef, ParseError> {
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
    /// (`pkg`, `core`, `self`, `super`; `platform` is a plain identifier).
    fn parse_use_segment(&mut self) -> Result<CstIdent, ParseError> {
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
                    expected: "a path segment (identifier, pkg, core, platform, self, or super)"
                        .into(),
                    found: format!("{:?}", self.current_kind()),
                },
                self.current().span,
            )),
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

        // A path may be rooted at a keyword (`pkg::m::T`, `core::List`,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cst::{CstBinaryOp, CstExprKind, CstUnaryOp};

    #[test]
    fn test_parse_number() {
        let mut parser = Parser::new("42").unwrap();
        let expr = parser.parse_expression().expect("parse error");
        assert!(matches!(expr.kind, CstExprKind::Number(n) if n == 42.0));
    }

    #[test]
    fn test_parse_string() {
        let mut parser = Parser::new(r#""hello""#).unwrap();
        let expr = parser.parse_expression().expect("parse error");
        assert!(matches!(&expr.kind, CstExprKind::String(s) if &**s == "hello"));
    }

    #[test]
    fn test_parse_bool() {
        let mut parser = Parser::new("true").unwrap();
        let expr = parser.parse_expression().expect("parse error");
        assert!(matches!(expr.kind, CstExprKind::Bool(true)));

        let mut parser = Parser::new("false").unwrap();
        let expr = parser.parse_expression().expect("parse error");
        assert!(matches!(expr.kind, CstExprKind::Bool(false)));
    }

    #[test]
    fn test_parse_binary_expr() {
        let mut parser = Parser::new("1 + 2 * 3").unwrap();
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
        let mut parser = Parser::new("-42").unwrap();
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
    fn test_struct_and_type_alias_split() {
        // `struct` defines a record.
        let mut parser = Parser::new("struct Point { x: Number, y: Number }").unwrap();
        let item = parser.parse_item().expect("struct should parse");
        assert!(matches!(item.kind, CstItemKind::TypeAlias(_)));

        // `unique(...) struct` defines a nominal record.
        let mut parser =
            Parser::new("unique(D098767B-4093-4D5C-BA37-AD92AA7B5D98) struct Id { value: Number }")
                .unwrap();
        let item = parser.parse_item().expect("unique struct should parse");
        assert!(matches!(item.kind, CstItemKind::TypeAlias(_)));

        // `type X = Y` remains a plain alias.
        let mut parser = Parser::new("type Meters = Number;").unwrap();
        let item = parser.parse_item().expect("type alias should parse");
        assert!(matches!(item.kind, CstItemKind::TypeAlias(_)));

        // The old record-via-`type` form is now a parse error (requires `=`).
        let mut parser = Parser::new("type Point { x: Number }").unwrap();
        assert!(parser.parse_item().is_err(), "`type Name {{ }}` must error");
    }

    #[test]
    fn test_parse_if_expr() {
        let mut parser = Parser::new("if x { 1 } else { 2 }").unwrap();
        let expr = parser.parse_expression().expect("parse error");
        assert!(matches!(expr.kind, CstExprKind::If { .. }));
    }

    #[test]
    fn test_parse_lambda() {
        let mut parser = Parser::new("(x) => x + 1").unwrap();
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
        let source = "fn add(x: Number, y: Number): Number { x + y }";
        let mut parser = Parser::new(source).unwrap();
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
        let source = "pub fn run(): () { () }";
        let mut parser = Parser::new(source).unwrap();
        let module = parser.parse_module().expect("parse error");
        assert_eq!(module.items.len(), 1);
        match &module.items[0].kind {
            CstItemKind::Function(f) => {
                assert_eq!(&*f.name.name, "run");
                assert!(f.is_public);
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_parse_function_with_abilities() {
        let source = "fn read_file(path: String): String with Filesystem { path }";
        let mut parser = Parser::new(source).unwrap();
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
        let mut parser = Parser::new(source).unwrap();
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
        let source = "ability Console { fn print(message: String): (); }";
        let mut parser = Parser::new(source).unwrap();
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
        let mut parser = Parser::new("{ x: 1, y: 2 }").unwrap();
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
        let mut parser = Parser::new("[1, 2, 3]").unwrap();
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
        let mut parser = Parser::new(source).unwrap();
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
        let mut parser = Parser::new("(1, 2, 3)").unwrap();
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
        let mut parser = Parser::new("()").unwrap();
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
        let mut parser = Parser::new(source).unwrap();
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
        let mut parser = Parser::new(source).unwrap();
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
        let mut parser = Parser::new(source).unwrap();
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
        let mut parser = Parser::new(source).unwrap();
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
        let mut parser = Parser::new(source).unwrap();
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
        let mut parser = Parser::new(source).unwrap();
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
        let mut parser = Parser::new(source).unwrap();
        let expr = parser.parse_expression().expect("parse error");
        match expr.kind {
            CstExprKind::Sandbox(sandbox) => {
                assert_eq!(sandbox.allowed_abilities.len(), 1);
                assert_eq!(&*sandbox.allowed_abilities[0].segments[0].name, "Log");
            }
            _ => panic!("Expected sandbox expression"),
        }
    }

    /// Parse a module expected to contain use items, and flatten them
    /// through lowering (the semantic surface tests care about).
    fn flatten_uses(source: &str) -> Vec<ambient_engine::ast::UseDef> {
        let mut parser = Parser::new(source).unwrap();
        let module = parser.parse_module().expect("parse error");
        let lowered = crate::lower::lower_module(&module).expect("lower error");
        lowered
            .items
            .into_iter()
            .filter_map(|item| match item.kind {
                ambient_engine::ast::ItemKind::Use(u) => Some(u),
                _ => None,
            })
            .collect()
    }

    fn path_names(u: &ambient_engine::ast::UseDef) -> Vec<&str> {
        u.path.iter().map(|(name, _)| name.as_ref()).collect()
    }

    #[test]
    fn test_parse_use_pkg_module() {
        let uses = flatten_uses("use pkg::utils;");
        assert_eq!(uses.len(), 1);
        assert!(!uses[0].is_public);
        assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Pkg);
        assert_eq!(path_names(&uses[0]), ["utils"]);
        assert!(uses[0].alias.is_none());
    }

    #[test]
    fn test_parse_use_pkg_nested() {
        let uses = flatten_uses("use pkg::utils::format;");
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Pkg);
        assert_eq!(path_names(&uses[0]), ["utils", "format"]);
    }

    #[test]
    fn test_parse_use_pkg_items() {
        // Braces are pure grouping: the tree flattens to one UseDef per leaf.
        let uses = flatten_uses("use pkg::utils::{format, parse};");
        assert_eq!(uses.len(), 2);
        assert_eq!(path_names(&uses[0]), ["utils", "format"]);
        assert_eq!(path_names(&uses[1]), ["utils", "parse"]);
    }

    #[test]
    fn test_parse_use_nested_groups() {
        let uses = flatten_uses("use pkg::a::{b::c, d::{e, f as g}};");
        assert_eq!(uses.len(), 3);
        assert_eq!(path_names(&uses[0]), ["a", "b", "c"]);
        assert_eq!(path_names(&uses[1]), ["a", "d", "e"]);
        assert_eq!(path_names(&uses[2]), ["a", "d", "f"]);
        assert_eq!(uses[2].alias.as_ref().map(|(n, _)| n.as_ref()), Some("g"));
        assert_eq!(uses[2].local_name().map(AsRef::as_ref), Some("g"));
    }

    #[test]
    fn test_parse_use_root_group() {
        let uses = flatten_uses("use {core::math, platform::Stdio};");
        assert_eq!(uses.len(), 2);
        assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Core);
        assert_eq!(path_names(&uses[0]), ["math"]);
        assert_eq!(uses[1].prefix, ambient_engine::ast::UsePrefix::Platform);
        assert_eq!(path_names(&uses[1]), ["Stdio"]);
    }

    #[test]
    fn test_parse_use_alias() {
        let uses = flatten_uses("use core::math::sqrt as root2;");
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Core);
        assert_eq!(path_names(&uses[0]), ["math", "sqrt"]);
        assert_eq!(uses[0].local_name().map(AsRef::as_ref), Some("root2"));
    }

    #[test]
    fn test_parse_use_local_root() {
        // A path rooted at a module alias from another use.
        let uses = flatten_uses("use pkg::deep::nested;\nuse nested::leaf::f;");
        assert_eq!(uses.len(), 2);
        assert_eq!(uses[1].prefix, ambient_engine::ast::UsePrefix::Local);
        assert_eq!(path_names(&uses[1]), ["nested", "leaf", "f"]);
    }

    #[test]
    fn test_parse_use_super_chain() {
        let uses = flatten_uses("use super::super::m::f;");
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Super(2));
        assert_eq!(path_names(&uses[0]), ["m", "f"]);
    }

    #[test]
    fn test_parse_use_keyword_mid_path_is_error() {
        let source = "use pkg::a::core::b;";
        let mut parser = Parser::new(source).unwrap();
        let module = parser.parse_module().expect("parse ok");
        assert!(crate::lower::lower_module(&module).is_err());
    }

    #[test]
    fn test_parse_use_in_block() {
        let source = "fn f(): Number {\n  use core::math::sqrt;\n  sqrt(16)\n}";
        let mut parser = Parser::new(source).unwrap();
        let module = parser.parse_module().expect("parse error");
        let lowered = crate::lower::lower_module(&module).expect("lower error");
        let ambient_engine::ast::ItemKind::Function(f) = &lowered.items[0].kind else {
            panic!("expected function");
        };
        let ambient_engine::ast::ExprKind::Block(stmts, result) = &f.body.kind else {
            panic!("expected block body");
        };
        assert!(matches!(
            stmts[0].kind,
            ambient_engine::ast::StmtKind::Use(_)
        ));
        assert!(result.is_some());
    }

    #[test]
    fn test_parse_use_platform() {
        // `platform` is a contextual keyword in use-root position.
        let uses = flatten_uses("use platform::Network;");
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Platform);
        assert_eq!(path_names(&uses[0]), ["Network"]);
    }

    #[test]
    fn test_parse_use_pkg_named_platform() {
        // A user path segment `platform` under `pkg` is still `Pkg` — the
        // contextual keyword only wins in root position.
        let uses = flatten_uses("use pkg::platform;");
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Pkg);
        assert_eq!(path_names(&uses[0]), ["platform"]);
    }

    #[test]
    fn test_with_platform_stays_qualified_name() {
        // Regression: the soft `platform` keyword must NOT leak into
        // `parse_qualified_name`, so a `with platform::Network` ability head
        // still parses as a two-segment qualified name starting with
        // `platform`.
        let source = "fn f(): () with platform::Network { () }";
        let mut parser = Parser::new(source).unwrap();
        let module = parser.parse_module().expect("parse error");
        match &module.items[0].kind {
            CstItemKind::Function(f) => {
                assert_eq!(f.abilities.len(), 1);
                let segments = &f.abilities[0].segments;
                assert_eq!(segments.len(), 2);
                assert_eq!(&*segments[0].name, "platform");
                assert_eq!(&*segments[1].name, "Network");
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_parse_use_self() {
        let uses = flatten_uses("use self::sibling;");
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Self_);
        assert_eq!(path_names(&uses[0]), ["sibling"]);
    }

    #[test]
    fn test_parse_use_super() {
        let uses = flatten_uses("use super::parent;");
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Super(1));
        assert_eq!(path_names(&uses[0]), ["parent"]);
    }

    #[test]
    fn test_parse_use_super_super() {
        let uses = flatten_uses("use super::super::grandparent;");
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Super(2));
        assert_eq!(path_names(&uses[0]), ["grandparent"]);
    }

    #[test]
    fn test_parse_pub_use() {
        let uses = flatten_uses("pub use pkg::utils;");
        assert_eq!(uses.len(), 1);
        assert!(uses[0].is_public);
        assert_eq!(uses[0].prefix, ambient_engine::ast::UsePrefix::Pkg);
    }
}

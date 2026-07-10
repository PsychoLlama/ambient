//! Item parsing: top-level items, functions, and the various definition
//! forms (`const`, `struct`, `enum`, `ability`, `trait`, `impl`).

use std::sync::Arc;

use ambient_engine::ast::Span;

use super::Parser;
use crate::cst::{
    CstAbilityDef, CstAbilityMethod, CstConstDef, CstEnumDef, CstEnumVariant, CstExternFnDef,
    CstFunctionDef, CstImplDef, CstImplMethod, CstItem, CstItemKind, CstParam, CstStructDef,
    CstTraitDef, CstTraitMethod, CstTraitParam, CstTraitParamKind, CstTypeAliasDef, CstTypeParam,
    CstWhereClause,
};
use crate::error::{ParseError, ParseErrorKind};
use crate::lexer::TokenKind;

impl Parser<'_> {
    /// Parse a top-level item.
    pub(super) fn parse_item(&mut self) -> Result<CstItem, ParseError> {
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
                    TokenKind::Struct => {
                        CstItemKind::Struct(self.parse_struct_def(true, None, false)?)
                    }
                    TokenKind::Enum => CstItemKind::Enum(self.parse_enum(true, None)?),
                    TokenKind::Unique => self.parse_unique_item(true, false)?,
                    TokenKind::Extern => self.parse_extern_item(true)?,
                    TokenKind::Ability => CstItemKind::Ability(self.parse_ability_def(true, None)?),
                    TokenKind::Trait => CstItemKind::Trait(self.parse_trait_def(true, None)?),
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
            TokenKind::Struct => CstItemKind::Struct(self.parse_struct_def(false, None, false)?),
            TokenKind::Enum => CstItemKind::Enum(self.parse_enum(false, None)?),
            TokenKind::Unique => self.parse_unique_item(false, false)?,
            TokenKind::Extern => self.parse_extern_item(false)?,
            TokenKind::Ability => CstItemKind::Ability(self.parse_ability_def(false, None)?),
            TokenKind::Use => CstItemKind::Use(self.parse_use(false)?),
            TokenKind::Trait => CstItemKind::Trait(self.parse_trait_def(false, None)?),
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

    /// Parse a `<...>` type-parameter list. Each parameter is a name
    /// (`T`), an ability variable (`E!`), or a bounded parameter
    /// (`T: Eq + Ord`) — trait bounds are `+`-separated qualified names,
    /// exactly the `where` grammar inlined at the declaration site.
    fn parse_type_params(&mut self) -> Result<Vec<CstTypeParam>, ParseError> {
        self.expect(TokenKind::Lt)?;
        let mut params = Vec::new();

        loop {
            if self.check(TokenKind::Gt) {
                break;
            }

            let name = self.parse_ident()?;
            let is_ability = self.consume(TokenKind::Bang).is_some();

            let mut bounds = Vec::new();
            if self.consume(TokenKind::Colon).is_some() {
                loop {
                    bounds.push(self.parse_qualified_name()?);
                    if self.consume(TokenKind::Plus).is_none() {
                        break;
                    }
                }
            }

            let end = bounds.last().map_or(name.span.end, |b| b.span.end);
            let span = Span::new(name.span.start, end);

            params.push(CstTypeParam {
                name,
                is_ability,
                bounds,
                span,
            });

            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }

        self.expect(TokenKind::Gt)?;
        Ok(params)
    }

    pub(super) fn parse_params(&mut self) -> Result<Vec<CstParam>, ParseError> {
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

    pub(super) fn parse_const(&mut self, is_public: bool) -> Result<CstConstDef, ParseError> {
        self.expect(TokenKind::Const)?;
        let name = self.parse_ident()?;
        // The type annotation is optional: when omitted the checker infers it
        // from the literal initializer (`const MAX = 100;`).
        let ty = if self.consume(TokenKind::Colon).is_some() {
            Some(self.parse_type()?)
        } else {
            None
        };
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
    fn parse_unique_item(
        &mut self,
        is_public: bool,
        is_extern: bool,
    ) -> Result<CstItemKind, ParseError> {
        let unique_id = self.parse_unique_prefix()?;
        self.skip_trivia();
        match self.current_kind() {
            TokenKind::Struct => Ok(CstItemKind::Struct(
                self.parse_struct_def(is_public, unique_id, is_extern)?,
            )),
            TokenKind::Enum | TokenKind::Ability if is_extern => Err(ParseError::new(
                ParseErrorKind::Expected {
                    expected:
                        "`struct` after `extern unique(...)` (`extern` applies to `struct` only)"
                            .into(),
                    found: format!("{:?}", self.current_kind()),
                },
                self.current().span,
            )),
            TokenKind::Enum => Ok(CstItemKind::Enum(self.parse_enum(is_public, unique_id)?)),
            TokenKind::Ability => Ok(CstItemKind::Ability(
                self.parse_ability_def(is_public, unique_id)?,
            )),
            TokenKind::Trait => Ok(CstItemKind::Trait(
                self.parse_trait_def(is_public, unique_id)?,
            )),
            other => Err(ParseError::new(
                ParseErrorKind::Expected {
                    expected: "`struct`, `enum`, `ability`, or `trait` after `unique(...)`".into(),
                    found: format!("{other:?}"),
                },
                self.current().span,
            )),
        }
    }

    /// Parse an `extern`-prefixed item. Two forms exist:
    ///
    /// - `extern unique(<uuid>) struct T ...` marks a nominal type as
    ///   host-provided — user code may name it and read its fields, but not
    ///   construct it. It requires `unique(...)` (the type's identity must be
    ///   readable from source alone, so checking works without host bindings)
    ///   and applies to `struct` only. The requirement is re-checked in
    ///   lowering.
    /// - `extern fn name(...): Ret;` declares a body-less function whose
    ///   implementation (and stable UUID identity) the host binds at compile
    ///   time.
    fn parse_extern_item(&mut self, is_public: bool) -> Result<CstItemKind, ParseError> {
        self.expect(TokenKind::Extern)?;
        self.skip_trivia();
        match self.current_kind() {
            TokenKind::Fn => Ok(CstItemKind::ExternFn(self.parse_extern_fn(is_public)?)),
            TokenKind::Unique => self.parse_unique_item(is_public, true),
            other => Err(ParseError::new(
                ParseErrorKind::Expected {
                    expected: "`fn` or `unique(<uuid>)` after `extern`".into(),
                    found: format!("{other:?}"),
                },
                self.current().span,
            )),
        }
    }

    /// Parse the signature of an `extern fn` declaration: like a function
    /// header, but terminated by `;` instead of a body. A `with` clause is
    /// rejected here — extern fns are pure by construction (effectful host
    /// integration is what abilities are for).
    fn parse_extern_fn(&mut self, is_public: bool) -> Result<CstExternFnDef, ParseError> {
        self.expect(TokenKind::Fn)?;
        let name = self.parse_ident()?;

        let type_params = if self.check(TokenKind::Lt) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };

        self.expect(TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(TokenKind::RParen)?;

        let ret_ty = if self.consume(TokenKind::Colon).is_some() {
            Some(self.parse_type()?)
        } else {
            None
        };

        if self.check(TokenKind::With) {
            return Err(ParseError::new(
                ParseErrorKind::ExternFnWithAbilities,
                self.current().span,
            ));
        }

        self.expect(TokenKind::Semi)?;

        Ok(CstExternFnDef {
            is_public,
            name,
            type_params,
            params,
            ret_ty,
        })
    }

    /// Parse a `struct Foo { fields }` record definition, or the unit form
    /// `struct Foo;` (a fieldless nominal type). There is no `= Type` alias form
    /// (that is `parse_type_alias`).
    fn parse_struct_def(
        &mut self,
        is_public: bool,
        unique_id: Option<Arc<str>>,
        is_extern: bool,
    ) -> Result<CstStructDef, ParseError> {
        self.expect(TokenKind::Struct)?;
        let name = self.parse_ident()?;

        let type_params = if self.check(TokenKind::Lt) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };

        // A trailing `;` marks a unit struct (no body); otherwise the body is a
        // record type. Lowering enforces the `unique(...)` and non-empty rules.
        let ty = if self.consume(TokenKind::Semi).is_some() {
            None
        } else {
            Some(self.parse_record_type()?)
        };

        Ok(CstStructDef {
            is_public,
            name,
            type_params,
            ty,
            unique_id,
            is_extern,
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

    fn parse_ability_def(
        &mut self,
        is_public: bool,
        unique_id: Option<Arc<str>>,
    ) -> Result<CstAbilityDef, ParseError> {
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
            unique_id,
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

        // A block is the method's default implementation; a `;` leaves it
        // abstract (rejected later everywhere but the Exception carve-out).
        let (body, end) = if self.check(TokenKind::LBrace) {
            let body = self.parse_block_expr()?;
            let end = body.span.end;
            (Some(body), end)
        } else {
            let end = self.expect(TokenKind::Semi)?.span.end;
            (None, end)
        };

        Ok(CstAbilityMethod {
            name,
            type_params,
            params,
            ret_ty,
            body,
            span: Span::new(start, end),
        })
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Trait parsing
    // ─────────────────────────────────────────────────────────────────────────

    /// Parse a trait definition:
    /// `unique(<uuid>) trait Name<T> with Supertrait { fn method(self, ...): RetType; }`
    ///
    /// The `unique(...)` prefix was consumed by [`Self::parse_unique_item`]
    /// and arrives as `unique_id`; a bare `trait` still parses (with `None`)
    /// so lowering can report the mandatory-identity error precisely.
    fn parse_trait_def(
        &mut self,
        is_public: bool,
        unique_id: Option<Arc<str>>,
    ) -> Result<CstTraitDef, ParseError> {
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
            unique_id,
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
}

//! `set` declaration parsing: named ability sets and the row-expression
//! algebra (`Union`/`Difference` combinators) on their right-hand side.

use super::Parser;
use crate::cst::{CstRowExpr, CstSetDef};
use crate::error::ParseError;
use crate::lexer::TokenKind;

impl Parser<'_> {
    /// Parse a `set` declaration: `set IO = Stdio, FileSystem, Tcp;`. The
    /// contextual `set` keyword (an `Ident`) has already been recognized by
    /// the caller but not consumed.
    pub(super) fn parse_set(&mut self, is_public: bool) -> Result<CstSetDef, ParseError> {
        self.advance(); // consume the contextual `set` keyword
        self.skip_trivia();
        let name = self.parse_ident()?;
        self.expect(TokenKind::Eq)?;
        self.skip_trivia();
        let body = self.parse_row_expr()?;
        self.expect(TokenKind::Semi)?;
        Ok(CstSetDef {
            is_public,
            name,
            body,
        })
    }

    /// Parse a row expression — a `set`'s right-hand side. A top-level comma
    /// list is a union; each element is an ability/set name or a
    /// `Union<A, B>` / `Difference<A, B>` combinator.
    fn parse_row_expr(&mut self) -> Result<CstRowExpr, ParseError> {
        let mut parts = vec![self.parse_row_atom()?];
        while self.consume(TokenKind::Comma).is_some() {
            self.skip_trivia();
            parts.push(self.parse_row_atom()?);
        }
        Ok(if parts.len() == 1 {
            parts.pop().expect("non-empty")
        } else {
            CstRowExpr::Union(parts)
        })
    }

    /// Parse one row atom: an ability/set name, or a binary combinator.
    /// Combinator arguments are themselves atoms (not comma lists), so a
    /// union inside an argument is spelled `Union<A, B>` or named as a set.
    fn parse_row_atom(&mut self) -> Result<CstRowExpr, ParseError> {
        let name = self.parse_qualified_name()?;
        let combinator = (name.segments.len() == 1).then(|| &*name.segments[0].name);
        if self.check(TokenKind::Lt) && matches!(combinator, Some("Union" | "Difference")) {
            let is_union = combinator == Some("Union");
            self.advance(); // consume `<`
            self.skip_trivia();
            let a = self.parse_row_atom()?;
            self.expect(TokenKind::Comma)?;
            self.skip_trivia();
            let b = self.parse_row_atom()?;
            self.expect(TokenKind::Gt)?;
            self.skip_trivia();
            return Ok(if is_union {
                CstRowExpr::Union(vec![a, b])
            } else {
                CstRowExpr::Difference(Box::new(a), Box::new(b))
            });
        }
        Ok(CstRowExpr::Name(name))
    }
}

//! A provide: the one construct that takes a body and is written with
//! a word of its own. The others are calls of intrinsics.

use super::super::lex::Token;
use super::{Parsed, Parser};
use crate::surface::Expr;

impl Parser {
    /// What follows `provide`: `(restricted)` if the bundle is all the
    /// body has, the bundle, and the body. The word is reserved, so
    /// the brackets hold no bundle of that name.
    pub(super) fn provide(&mut self) -> Parsed<Expr> {
        let restricted =
            self.at("(") && matches!(self.peek_at(1), Token::Word(word) if word == "restricted");
        if restricted {
            self.advance();
            self.advance();
            self.expect(")")?;
        }
        Ok(Expr::Provide {
            bundle: Box::new(self.expr()?),
            restricted,
            body: Box::new(self.body()?),
        })
    }
}

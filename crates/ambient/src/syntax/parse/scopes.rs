//! A provide: the one construct that takes a body and is written with
//! a word of its own. The others are calls of intrinsics.

use super::{Parsed, Parser};
use crate::surface::Expr;

impl Parser {
    /// What follows `provide`: `only` if the bundle is all the body
    /// has, the bundle, and the body.
    pub(super) fn provide(&mut self) -> Parsed<Expr> {
        let only = self.eat_word("only");
        Ok(Expr::Provide {
            bundle: Box::new(self.expr()?),
            only,
            body: Box::new(self.body()?),
        })
    }
}

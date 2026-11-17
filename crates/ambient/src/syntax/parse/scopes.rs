//! The constructs that take a body: a boundary, a provide, a
//! recording, a group and its tasks; and the joins and cells.

use super::super::lex::Token;
use super::{Parsed, Parser};
use crate::builder as b;
use crate::surface::Expr;

impl Parser {
    /// The construct `word` begins.
    pub(super) fn scope(&mut self, word: &str) -> Parsed<Expr> {
        self.advance();
        match word {
            "try" => Ok(b::boundary(self.body()?)),
            "group" => Ok(b::group(self.body()?)),
            "spawn" => Ok(b::spawn(self.body()?)),
            "provide" => {
                let only = self.eat_word("only");
                Ok(Expr::Provide {
                    bundle: Box::new(self.expr()?),
                    only,
                    body: Box::new(self.body()?),
                })
            }
            "observe" => {
                let observer = self.expr()?;
                Ok(b::recording(observer, self.body()?))
            }
            "all" => Ok(b::all(self.branches()?)),
            "first" => Ok(b::first(self.branches()?)),
            _ => {
                self.expect("<")?;
                let ty = self.ty()?;
                self.expect(">")?;
                self.expect("(")?;
                let value = self.expr()?;
                self.expect(")")?;
                Ok(b::cell(ty, value))
            }
        }
    }

    /// The branches of an all or a first: each a function of no
    /// parameters, whose body is the branch.
    fn branches(&mut self) -> Parsed<Vec<Expr>> {
        self.expect("(")?;
        self.list(")", |parser| {
            let written = parser.at_word("fn")
                && matches!(parser.peek_at(1), Token::Mark("("))
                && matches!(parser.peek_at(2), Token::Mark(")"));
            if written {
                (0..3).for_each(|_| drop(parser.advance()));
                return parser.body();
            }
            // Any other function is called to make its branch.
            Ok(b::call(parser.expr()?, []))
        })
    }
}

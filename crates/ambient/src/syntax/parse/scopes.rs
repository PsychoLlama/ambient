//! The constructs that take a body: a boundary, a budget, a provide, a
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
            "budget" => self.budget(),
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

    /// `(steps: n, allocation: n)` and a body. Either limit may come
    /// first.
    fn budget(&mut self) -> Parsed<Expr> {
        self.expect("(")?;
        let (mut steps, mut allocation) = (None, None);
        let limits = self.list(")", |parser| {
            let at = parser.position();
            let limit = parser.name()?;
            parser.expect(":")?;
            Ok((at, limit, parser.expr()?))
        })?;
        for (at, limit, value) in limits {
            let slot = match limit.as_str() {
                "steps" => &mut steps,
                "allocation" => &mut allocation,
                _ => return self.fail_at(at, "a budget limits `steps` and `allocation`"),
            };
            if slot.replace(value).is_some() {
                return self.fail_at(at, format!("`{limit}` is limited twice"));
            }
        }
        let (Some(steps), Some(allocation)) = (steps, allocation) else {
            return self.fail("a budget limits both `steps` and `allocation`");
        };
        Ok(b::budget(steps, allocation, self.body()?))
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

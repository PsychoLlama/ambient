//! Record literals.

use super::super::lex::Token;
use super::expr::name;
use super::{Parsed, Parser};
use crate::surface::{Expr, Key};

/// One thing written between a record's braces.
enum Member {
    Field(Key, Expr),
    /// `...record`
    Spread(Expr),
}

impl Parser {
    /// A record, from its brace. Whatever it spreads is merged, in the
    /// order written, with the fields around it.
    pub(super) fn record(&mut self) -> Parsed<Expr> {
        self.expect("{")?;
        let members = self.list("}", Self::member)?;
        let mut parts: Vec<Expr> = Vec::new();
        let mut fields = Vec::new();
        for member in members {
            match member {
                Member::Field(key, value) => fields.push((key, value)),
                Member::Spread(record) => {
                    if !fields.is_empty() {
                        parts.push(Expr::Record(std::mem::take(&mut fields)));
                    }
                    parts.push(record);
                }
            }
        }
        if !fields.is_empty() || parts.is_empty() {
            parts.push(Expr::Record(fields));
        }
        let merged = parts.into_iter().reduce(|left, right| Expr::Merge {
            left: Box::new(left),
            right: Box::new(right),
        });
        Ok(merged.unwrap_or(Expr::Record(Vec::new())))
    }

    fn member(&mut self) -> Parsed<Member> {
        if self.eat("...") {
            return Ok(Member::Spread(self.expr()?));
        }
        let key = if self.eat("[") {
            let path = self.path()?;
            self.expect("]")?;
            Key::Ability(path)
        } else {
            let field = self.name()?;
            // A name alone is the local of that name.
            if matches!(self.peek(), Token::Mark("," | "}")) {
                let value = name(&field);
                return Ok(Member::Field(Key::Field(field), value));
            }
            Key::Field(field)
        };
        if self.eat(":") {
            return Ok(Member::Field(key, self.expr()?));
        }
        if !self.at("(") && !self.at("<") {
            return self.unexpected("`:` and a value, or parameters");
        }
        let written = self.written()?;
        let method = written.is_method();
        let function = written.lambda();
        let value = if method {
            Expr::Method(Box::new(function))
        } else {
            function
        };
        Ok(Member::Field(key, value))
    }
}

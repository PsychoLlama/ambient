//! Blocks: statements, and the expression they are.
//!
//! The language has no statements and no way to leave a function early
//! (`spec/syntax.md`). A block is read as statements and then arranged:
//! each `let` scopes over what follows it, and what follows a branch
//! that may return becomes what its other branches go on to do.

use super::super::lex::Token;
use super::expr::{choose, name};
use super::patterns::Mode;
use super::{Parsed, Parser};
use crate::surface::{Arm, Expr, Pattern};
use crate::syntax::Position;

#[derive(Clone)]
enum Statement {
    Let(Pattern, Expr),
    /// A binding whose value is released when the block exits.
    Using(Pattern, Expr),
    Expr(Expr),
    Return(Expr, Position),
    /// An `if` or a match whose arms are blocks.
    Branch(Expr, Vec<(Pattern, Vec<Statement>)>),
}

/// Whether running statements ends in a `return`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Returns {
    Never,
    Sometimes,
    Always,
}

fn returns(statements: &[Statement]) -> Returns {
    let of = |statement: &Statement| match statement {
        Statement::Return(..) => Returns::Always,
        Statement::Branch(_, arms) => {
            let arms: Vec<Returns> = arms.iter().map(|(_, arm)| returns(arm)).collect();
            if arms.iter().all(|arm| *arm == Returns::Always) {
                Returns::Always
            } else if arms.iter().all(|arm| *arm == Returns::Never) {
                Returns::Never
            } else {
                Returns::Sometimes
            }
        }
        Statement::Let(..) | Statement::Using(..) | Statement::Expr(_) => Returns::Never,
    };
    match statements.split_last() {
        Some((last, _)) if of(last) == Returns::Always => Returns::Always,
        _ if statements.iter().all(|each| of(each) == Returns::Never) => Returns::Never,
        _ => Returns::Sometimes,
    }
}

impl Parser {
    /// A block, as the expression it is.
    pub(super) fn block(&mut self) -> Parsed<Expr> {
        let statements = self.statements()?;
        self.arrange(statements)
    }

    /// The statements between braces. What they bind is in scope to the
    /// closing brace.
    fn statements(&mut self) -> Parsed<Vec<Statement>> {
        self.expect("{")?;
        self.scoped(|parser| {
            let mut statements = Vec::new();
            while !parser.eat("}") {
                statements.push(parser.statement()?);
            }
            Ok(statements)
        })
    }

    fn statement(&mut self) -> Parsed<Statement> {
        let at = self.position();
        let using = self.at_word("using");
        if using || self.at_word("let") {
            self.advance();
            let mut binds = Vec::new();
            let pattern = self.pattern(Mode::Binding, &mut binds)?;
            self.expect("=")?;
            let value = self.expr()?;
            self.expect(";")?;
            self.locals.extend(binds);
            return Ok(if using {
                Statement::Using(pattern, value)
            } else {
                Statement::Let(pattern, value)
            });
        }
        if self.eat_word("return") {
            let value = if self.at(";") {
                Expr::Null
            } else {
                self.expr()?
            };
            self.expect(";")?;
            return Ok(Statement::Return(value, at));
        }
        if self.eat_word("if") {
            return self.conditional();
        }
        if self.eat_word("match") {
            return self.match_statement();
        }
        // A function in a block is a local that holds it.
        if self.at_word("fn") && self.peek_ahead_is_name() {
            self.advance();
            let function = self.name()?;
            let written = self.written()?.lambda();
            self.locals.push(function.clone());
            let binder = Pattern::Bind(function, Box::new(Pattern::Wildcard));
            return Ok(Statement::Let(binder, written));
        }
        // A construct that ends in its own block needs nothing after it.
        let scope = ["try", "budget", "provide", "observe", "group", "spawn"];
        let scope = scope.iter().any(|word| self.at_word(word));
        let value = self.expr()?;
        if !(scope && matches!(self.peek_back_one(), Token::Mark("}")) && !self.at(";")) {
            self.expect(";")?;
        }
        Ok(Statement::Expr(value))
    }

    fn peek_ahead_is_name(&self) -> bool {
        matches!(self.peek_at(1), Token::Word(_))
    }

    /// The token just taken.
    fn peek_back_one(&self) -> &Token {
        &self.tokens[self.index.saturating_sub(1)]
    }

    /// An `if`, after the word: a condition, a block, and maybe an
    /// `else` with a block or another `if`.
    fn conditional(&mut self) -> Parsed<Statement> {
        let condition = self.expr()?;
        let then = self.statements()?;
        let otherwise = if !self.eat_word("else") {
            Vec::new()
        } else if self.eat_word("if") {
            vec![self.conditional()?]
        } else {
            self.statements()?
        };
        let arm = |value| Pattern::Literal(crate::literal::Literal::Bool(value));
        Ok(Statement::Branch(
            condition,
            vec![(arm(true), then), (arm(false), otherwise)],
        ))
    }

    /// A match written as a statement, after the word: each arm is a
    /// block, or one expression.
    fn match_statement(&mut self) -> Parsed<Statement> {
        let subject = self.expr()?;
        self.expect("{")?;
        let mut arms = Vec::new();
        while !self.eat("}") {
            let mut binds = Vec::new();
            let pattern = self.pattern(Mode::Testing, &mut binds)?;
            self.expect("=>")?;
            let body = self.scoped(|parser| {
                parser.locals.extend(binds);
                if parser.at("{") {
                    parser.statements()
                } else {
                    Ok(vec![Statement::Expr(parser.expr()?)])
                }
            })?;
            arms.push((pattern, body));
            self.eat(",");
        }
        Ok(Statement::Branch(subject, arms))
    }

    /// The expression statements are, run in order.
    fn arrange(&mut self, mut statements: Vec<Statement>) -> Parsed<Expr> {
        if statements.is_empty() {
            return Ok(Expr::Null);
        }
        let rest = statements.split_off(1);
        let first = statements.remove(0);
        match first {
            Statement::Return(value, at) => {
                if rest.is_empty() {
                    Ok(value)
                } else {
                    self.fail_at(at, "what follows a `return` never runs")
                }
            }
            Statement::Let(Pattern::Bind(name, inner), value)
                if matches!(*inner, Pattern::Wildcard) =>
            {
                Ok(Expr::Let {
                    name,
                    value: Box::new(value),
                    body: Box::new(self.arrange(rest)?),
                })
            }
            Statement::Let(pattern, value) => Ok(Expr::Match {
                subject: Box::new(value),
                arms: vec![Arm {
                    pattern,
                    body: self.arrange(rest)?,
                }],
            }),
            Statement::Using(pattern, value) => Ok(Expr::Using {
                pattern,
                value: Box::new(value),
                body: Box::new(self.arrange(rest)?),
            }),
            Statement::Expr(value) => Ok(dropped(value, self.arrange(rest)?)),
            Statement::Branch(subject, arms) => self.branch(subject, arms, rest),
        }
    }

    /// A branch and the statements after it.
    fn branch(
        &mut self,
        subject: Expr,
        mut arms: Vec<(Pattern, Vec<Statement>)>,
        rest: Vec<Statement>,
    ) -> Parsed<Expr> {
        let falls = |arm: &(Pattern, Vec<Statement>)| returns(&arm.1) != Returns::Always;
        let falling = arms.iter().filter(|arm| falls(arm)).count();
        let returning = arms.iter().any(|arm| returns(&arm.1) != Returns::Never);
        // No arm returns: the branch is one statement among the others.
        if !rest.is_empty() && !returning {
            let branch = self.matched(subject, arms)?;
            return Ok(dropped(branch, self.arrange(rest)?));
        }
        // Some arm returns, so what follows belongs to the arms that do
        // not. More than one of them share it as a function they call.
        let mut shared = None;
        if !rest.is_empty() && falling > 1 {
            let then = self.made_up("then");
            let body = self.arrange(rest)?;
            let call = Expr::Call {
                callee: Box::new(name(&then)),
                args: Vec::new(),
            };
            let position = Position { line: 0, column: 0 };
            for arm in arms.iter_mut().filter(|arm| falls(arm)) {
                arm.1.push(Statement::Return(call.clone(), position));
            }
            shared = Some((then, body));
        } else if !rest.is_empty() {
            for arm in arms.iter_mut().filter(|arm| falls(arm)) {
                arm.1.extend(rest.iter().cloned());
            }
        }
        let branch = self.matched(subject, arms)?;
        Ok(match shared {
            None => branch,
            Some((then, body)) => Expr::Let {
                name: then,
                value: Box::new(Expr::Lambda {
                    type_params: Vec::new(),
                    params: Vec::new(),
                    row: Vec::new(),
                    returns: None,
                    body: Box::new(body),
                }),
                body: Box::new(branch),
            },
        })
    }

    /// A match of `subject` whose arms are blocks.
    fn matched(&mut self, subject: Expr, arms: Vec<(Pattern, Vec<Statement>)>) -> Parsed<Expr> {
        let arms = arms
            .into_iter()
            .map(|(pattern, body)| {
                let body = self.arrange(body)?;
                Ok(Arm { pattern, body })
            })
            .collect::<Parsed<Vec<Arm>>>()?;
        // An `if` is a match of a boolean, written as every other is.
        if let [first, second] = arms.as_slice()
            && first.pattern == Pattern::Literal(crate::literal::Literal::Bool(true))
            && second.pattern == Pattern::Literal(crate::literal::Literal::Bool(false))
        {
            return Ok(choose(subject, first.body.clone(), second.body.clone()));
        }
        Ok(Expr::Match {
            subject: Box::new(subject),
            arms,
        })
    }
}

/// `value`, evaluated for what it does, and then `rest`.
fn dropped(value: Expr, rest: Expr) -> Expr {
    Expr::Let {
        name: "_".to_owned(),
        value: Box::new(value),
        body: Box::new(rest),
    }
}

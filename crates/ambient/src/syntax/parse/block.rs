//! Blocks: statements, and the expression they are.
//!
//! The language has no statements and no way to leave a function early
//! (`spec/syntax.md`). A block is read as statements and then arranged:
//! each `let` scopes over what follows it, and what follows a branch
//! that may return becomes what its other branches go on to do.

use super::super::lex::Token;
use super::expr::{choose, name};
use super::patterns::Mode;
use super::{Own, Parsed, Parser};
use crate::surface::{Arm, Expr, Key, Pattern};
use crate::syntax::Position;

#[derive(Clone)]
enum Statement {
    /// A binding, and the token it began at.
    Let(usize, Pattern, Expr),
    /// A binding whose value is released when the block exits.
    Using(usize, Pattern, Expr),
    Expr(Expr),
    Return(Expr, Position),
    /// An `if` or a match whose arms are blocks, and the token it began
    /// at.
    Branch(usize, Expr, Vec<(Pattern, Vec<Statement>)>),
    /// A `let` with an `else`: a pattern the value may not fit, and
    /// arms for when it does not. It is only ever in a block as read,
    /// where it becomes a branch whose first arm is the rest of the
    /// block ([`guarded`]).
    Guard(usize, Pattern, Expr, Vec<(Pattern, Vec<Statement>)>),
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
        Statement::Branch(_, _, arms) => {
            let arms: Vec<Returns> = arms.iter().map(|(_, arm)| returns(arm)).collect();
            if arms.iter().all(|arm| *arm == Returns::Always) {
                Returns::Always
            } else if arms.iter().all(|arm| *arm == Returns::Never) {
                Returns::Never
            } else {
                Returns::Sometimes
            }
        }
        Statement::Let(..) | Statement::Using(..) | Statement::Expr(_) | Statement::Guard(..) => {
            Returns::Never
        }
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
            parser.guarded(statements)
        })
    }

    fn statement(&mut self) -> Parsed<Statement> {
        let at = self.position();
        let began = self.index;
        let using = self.at_word("using");
        if using || self.at_word("let") {
            self.advance();
            let mut binds = Vec::new();
            let pattern = self.pattern(Mode::Binding, &mut binds)?;
            self.expect("=")?;
            let value = self.expr()?;
            // What an `else` binds is not what the pattern binds.
            let otherwise = if !using && self.eat_word("else") {
                Some(self.arms()?)
            } else {
                None
            };
            self.expect(";")?;
            self.locals.extend(binds);
            if let Some(arms) = otherwise {
                return Ok(Statement::Guard(began, pattern, value, arms));
            }
            return Ok(if using {
                Statement::Using(began, pattern, value)
            } else {
                Statement::Let(began, pattern, value)
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
            return self.conditional(began);
        }
        if self.eat_word("match") {
            return self.match_statement(began);
        }
        if self.eat_word("for") {
            return self.for_each(began);
        }
        if self.at_word("fn") && self.peek_ahead_is_name() {
            self.advance();
            return self.function_in_block(began);
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

    /// A function in a block, after `fn`: a local that holds it.
    ///
    /// One that names itself cannot be a lambda, which is made before
    /// anything is bound to it. It is the one method of a record, read
    /// from that record: a method is handed its record, so the body
    /// reaches the function through it and nothing holds itself.
    fn function_in_block(&mut self, began: usize) -> Parsed<Statement> {
        let function = self.name()?;
        let receiver = self.made_up("own");
        self.calling.push(Own {
            local: self.locals.len(),
            receiver: receiver.clone(),
            used: false,
        });
        self.locals.push(function.clone());
        let written = self.written();
        let named_itself = self.calling.pop().is_some_and(|own| own.used);
        let mut written = written?;
        // One whose body is an expression ends as a statement does.
        self.eat(";");
        let value = if named_itself {
            written.params.insert(0, (receiver, None));
            let method = Expr::Method(Box::new(written.lambda()));
            Expr::Field {
                target: Box::new(Expr::Record(vec![(Key::Field(function.clone()), method)])),
                key: Key::Field(function.clone()),
            }
        } else {
            written.lambda()
        };
        let binder = Pattern::Bind(function, Box::new(Pattern::Wildcard));
        Ok(Statement::Let(began, binder, value))
    }

    /// A `for`, after the word: a pattern, `in`, what gives the items,
    /// and a block. It is `each` called on that with a function of the
    /// block, so nothing leaves it early.
    fn for_each(&mut self, began: usize) -> Parsed<Statement> {
        let mut binds = Vec::new();
        let pattern = self.pattern(Mode::Binding, &mut binds)?;
        self.expect_word("in")?;
        let items = self.expr()?;
        let at = self.position();
        let statements = self.scoped(|parser| {
            parser.locals.extend(binds);
            parser.statements()
        })?;
        if returns(&statements) != Returns::Never {
            return self.fail_at(
                at,
                "a `for` runs its block for every item: no `return` leaves it",
            );
        }
        let item = self.made_up("item");
        let body = Expr::Match {
            subject: Box::new(name(&item)),
            arms: vec![Arm {
                pattern,
                body: self.arrange(statements)?,
            }],
        };
        let each = Expr::Lambda {
            type_params: Vec::new(),
            params: vec![(item, None)],
            row: Vec::new(),
            returns: None,
            body: Box::new(body),
        };
        let sent = Expr::Send {
            target: Box::new(items),
            method: Key::Field("each".to_owned()),
            args: vec![each],
        };
        Ok(Statement::Expr(self.mark(began, sent)))
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
    fn conditional(&mut self, began: usize) -> Parsed<Statement> {
        let condition = self.expr()?;
        let then = self.statements()?;
        let otherwise = if !self.eat_word("else") {
            Vec::new()
        } else if self.at_word("if") {
            let began = self.index;
            self.advance();
            vec![self.conditional(began)?]
        } else {
            self.statements()?
        };
        let arm = |value| Pattern::Literal(crate::literal::Literal::Bool(value));
        Ok(Statement::Branch(
            began,
            condition,
            vec![(arm(true), then), (arm(false), otherwise)],
        ))
    }

    /// A match written as a statement, after the word: each arm is a
    /// block, or one expression.
    fn match_statement(&mut self, began: usize) -> Parsed<Statement> {
        let subject = self.expr()?;
        Ok(Statement::Branch(began, subject, self.arms()?))
    }

    /// Arms between braces, each a block or one expression.
    fn arms(&mut self) -> Parsed<Vec<(Pattern, Vec<Statement>)>> {
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
        Ok(arms)
    }

    /// The statements of a block, with each `let` that has an `else`
    /// as the branch it means: the value matched, the rest of the
    /// block where it fits the pattern, and the arms where it does not.
    /// Each of those arms leaves, since nothing follows it.
    fn guarded(&self, mut statements: Vec<Statement>) -> Parsed<Vec<Statement>> {
        let guard = |statement: &Statement| matches!(statement, Statement::Guard(..));
        let Some(at) = statements.iter().position(guard) else {
            return Ok(statements);
        };
        let rest = self.guarded(statements.split_off(at + 1))?;
        let Some(Statement::Guard(began, pattern, value, otherwise)) = statements.pop() else {
            return Ok(statements);
        };
        if otherwise
            .iter()
            .any(|(_, arm)| returns(arm) != Returns::Always)
        {
            let message = "each arm of the `else` of a `let` ends in a `return`";
            return self.fail_at(self.positions[began], message);
        }
        let mut arms = vec![(pattern, rest)];
        arms.extend(otherwise);
        statements.push(Statement::Branch(began, value, arms));
        Ok(statements)
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
            Statement::Let(began, Pattern::Bind(name, inner), value)
                if matches!(*inner, Pattern::Wildcard) =>
            {
                let bound = Expr::Let {
                    name,
                    value: Box::new(value),
                    body: Box::new(self.arrange(rest)?),
                };
                Ok(self.mark(began, bound))
            }
            Statement::Let(began, pattern, value) => {
                let arm = Arm {
                    pattern,
                    body: self.arrange(rest)?,
                };
                let matched = Expr::Match {
                    subject: Box::new(value),
                    arms: vec![arm],
                };
                Ok(self.mark(began, matched))
            }
            Statement::Using(began, Pattern::Bind(name, inner), value)
                if matches!(*inner, Pattern::Wildcard) =>
            {
                let bound = Expr::Using {
                    name,
                    value: Box::new(value),
                    body: Box::new(self.arrange(rest)?),
                };
                Ok(self.mark(began, bound))
            }
            // The value is held whole under a name of the reader's, and
            // the pattern names its parts.
            Statement::Using(began, pattern, value) => {
                let held = self.made_up("held");
                let arm = Arm {
                    pattern,
                    body: self.arrange(rest)?,
                };
                let bound = Expr::Using {
                    value: Box::new(value),
                    body: Box::new(Expr::Match {
                        subject: Box::new(super::expr::name(&held)),
                        arms: vec![arm],
                    }),
                    name: held,
                };
                Ok(self.mark(began, bound))
            }
            Statement::Expr(value) => Ok(dropped(value, self.arrange(rest)?)),
            Statement::Branch(began, subject, arms) => self.branch(began, subject, arms, rest),
            // A block as read holds none: see `guarded`.
            Statement::Guard(began, ..) => self.fail_at(
                self.positions[began],
                "a `let` with an `else` is in a block",
            ),
        }
    }

    /// A branch and the statements after it.
    fn branch(
        &mut self,
        began: usize,
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
            let branch = self.mark(began, branch);
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
        let branch = self.mark(began, branch);
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

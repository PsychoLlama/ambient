//! Expressions.

use super::super::lex::Token;
use super::patterns::Mode;
use super::{Parsed, Parser};
use crate::literal::Literal;
use crate::surface::{
    Arm, Expr, Key, Operator, Path, Pattern, Type, TypeParam, constant_name, fallback,
};

/// The binary operators, loosest first. Each level binds to the left,
/// but comparison, which does not chain.
const LEVELS: [&[&str]; 5] = [
    &["||"],
    &["&&"],
    &["==", "!=", "<=", ">=", "<", ">"],
    &["+", "-"],
    &["*", "/", "%"],
];
const COMPARISON: usize = 2;

/// A function as it is written: a lambda, a member of a record, or a
/// function in a block.
pub(super) struct Written {
    pub(super) type_params: Vec<TypeParam>,
    pub(super) params: Vec<(String, Option<Type>)>,
    pub(super) row: Vec<Path>,
    pub(super) returns: Option<Type>,
    pub(super) body: Expr,
}

impl Written {
    pub(super) fn lambda(self) -> Expr {
        Expr::Lambda {
            type_params: self.type_params,
            params: self.params,
            row: self.row,
            returns: self.returns,
            body: Box::new(self.body),
        }
    }

    pub(super) fn is_method(&self) -> bool {
        self.params.first().is_some_and(|(name, _)| name == "self")
    }
}

pub(super) fn name(name: &str) -> Expr {
    Expr::Name(Path(vec![name.to_owned()]))
}

fn boolean(value: bool) -> Expr {
    Expr::Literal(Literal::Bool(value))
}

/// `subject`, and what to give for each of true and false.
pub(super) fn choose(subject: Expr, when_true: Expr, when_false: Expr) -> Expr {
    let arm = |value, body| Arm {
        pattern: Pattern::Literal(Literal::Bool(value)),
        body,
    };
    Expr::Match {
        subject: Box::new(subject),
        arms: vec![arm(true, when_true), arm(false, when_false)],
    }
}

fn not(value: Expr) -> Expr {
    choose(value, boolean(false), boolean(true))
}

fn operation(operator: Operator, operands: Vec<Expr>) -> Expr {
    Expr::Operation { operator, operands }
}

impl Parser {
    pub(super) fn expr(&mut self) -> Parsed<Expr> {
        let mut value = self.binary(0)?;
        // `value ?? otherwise`, which is a match of what may be null.
        while self.at("??") {
            let at = self.index;
            self.advance();
            let otherwise = self.binary(0)?;
            self.made += 1;
            value = self.mark(at, fallback(value, self.made, otherwise));
        }
        let at = self.index;
        // `condition ? this : that`, which is a match of a boolean.
        if self.eat("?") {
            let when_true = self.expr()?;
            self.expect(":")?;
            let chosen = choose(value, when_true, self.expr()?);
            return Ok(self.mark(at, chosen));
        }
        Ok(value)
    }

    fn binary(&mut self, level: usize) -> Parsed<Expr> {
        let Some(marks) = LEVELS.get(level) else {
            return self.unary();
        };
        let mut left = self.binary(level + 1)?;
        while let Some(mark) = marks.iter().find(|mark| self.at(mark)) {
            let at = self.index;
            self.advance();
            let right = self.binary(level + 1)?;
            let applied = self.operate(mark, left, right);
            left = self.mark(at, applied);
            if level == COMPARISON {
                break;
            }
        }
        Ok(left)
    }

    /// What an operator means, in the operations the language has.
    fn operate(&mut self, mark: &str, left: Expr, right: Expr) -> Expr {
        let simple = match mark {
            "+" => Some(Operator::Add),
            "-" => Some(Operator::Subtract),
            "*" => Some(Operator::Multiply),
            "/" => Some(Operator::Divide),
            "%" => Some(Operator::Remainder),
            "==" => Some(Operator::Equal),
            "<" => Some(Operator::Less),
            _ => None,
        };
        if let Some(operator) = simple {
            return operation(operator, vec![left, right]);
        }
        match mark {
            "!=" => not(operation(Operator::Equal, vec![left, right])),
            ">=" => not(operation(Operator::Less, vec![left, right])),
            "&&" => choose(left, right, boolean(false)),
            "||" => choose(left, boolean(true), right),
            // `<` with its operands exchanged. The left one is still
            // evaluated first, so it is bound.
            _ => {
                let kept = self.made_up("left");
                let less = operation(Operator::Less, vec![right, name(&kept)]);
                Expr::Let {
                    name: kept,
                    value: Box::new(left),
                    body: Box::new(if mark == ">" { less } else { not(less) }),
                }
            }
        }
    }

    fn unary(&mut self) -> Parsed<Expr> {
        let at = self.index;
        if self.eat("!") {
            let negated = not(self.unary()?);
            return Ok(self.mark(at, negated));
        }
        if self.eat_word("comptime") {
            let call = Expr::Comptime(Box::new(self.unary()?));
            return Ok(self.mark(at, call));
        }
        if self.eat("-") {
            let negative = match self.peek() {
                Token::Int(value) => Literal::Int(-value),
                Token::Float(value) => Literal::float(-value),
                // Anything else is asked for its negative, which the
                // library says for each shape that has one.
                _ => {
                    let asked = crate::builder::send(self.unary()?, "negate", []);
                    return Ok(self.mark(at, asked));
                }
            };
            self.advance();
            let literal = self.mark(at, Expr::Literal(negative));
            return self.postfix(at, literal);
        }
        let primary = self.primary()?;
        let primary = self.mark(at, primary);
        self.postfix(at, primary)
    }

    /// Calls, fields, and method calls written after `target`, which
    /// began at the token numbered `start`. A call is at what it calls,
    /// and a member at its name.
    fn postfix(&mut self, start: usize, mut target: Expr) -> Parsed<Expr> {
        loop {
            if self.eat("(") {
                let call = Expr::Call {
                    callee: Box::new(target),
                    args: self.list(")", Self::expr)?,
                };
                target = self.mark(start, call);
                continue;
            }
            let at = self.index + 1;
            let key = if self.eat(".") {
                // Any word names a member, reserved or not.
                match self.advance() {
                    Token::Word(word) => Key::Field(word),
                    _ => return self.fail("expected the name of a member after `.`"),
                }
            } else if self.eat("[") {
                let path = self.path()?;
                self.expect("]")?;
                Key::Ability(path)
            } else {
                return Ok(target);
            };
            let member = if self.eat("(") {
                Expr::Send {
                    target: Box::new(target),
                    method: key,
                    args: self.list(")", Self::expr)?,
                }
            } else {
                Expr::Field {
                    target: Box::new(target),
                    key,
                }
            };
            target = self.mark(at, member);
        }
    }

    fn primary(&mut self) -> Parsed<Expr> {
        match self.peek().clone() {
            Token::Int(value) => Ok(self.literal_expr(Literal::Int(value))),
            Token::Float(value) => Ok(self.literal_expr(Literal::float(value))),
            Token::Text(text) => Ok(self.literal_expr(Literal::String(text))),
            Token::Bytes(bytes) => Ok(self.literal_expr(Literal::Bytes(bytes))),
            Token::Mark("(") => {
                let at = self.position();
                self.advance();
                let members = self.list(")", Self::expr)?;
                Ok(self.grouped(at, members)?.unwrap_or_else(Expr::Tuple))
            }
            Token::Mark("[") => {
                self.advance();
                Ok(Expr::List(self.list("]", Self::expr)?))
            }
            Token::Mark("{") => self.record(),
            Token::Word(word) => self.word(&word),
            _ => self.unexpected("an expression"),
        }
    }

    fn literal_expr(&mut self, literal: Literal) -> Expr {
        self.advance();
        Expr::Literal(literal)
    }

    /// An expression that begins with a word.
    fn word(&mut self, word: &str) -> Parsed<Expr> {
        match word {
            "true" => Ok(self.literal_expr(Literal::Bool(true))),
            "false" => Ok(self.literal_expr(Literal::Bool(false))),
            "null" => {
                self.advance();
                Ok(Expr::Null)
            }
            "fn" => {
                self.advance();
                Ok(self.written()?.lambda())
            }
            "match" => {
                self.advance();
                self.match_value()
            }
            "provide" => {
                self.advance();
                self.provide()
            }
            _ => self.named(),
        }
    }

    /// A name: a local, or a path to a definition, which may be
    /// performed or given type arguments.
    fn named(&mut self) -> Parsed<Expr> {
        if matches!(self.peek(), Token::Word(word) if self.is_local(word)) {
            let local = self.name()?;
            // A function in a block names itself through the record it
            // is a method of: see `Parser::function_in_block`.
            return Ok(match self.own_receiver(&local) {
                Some(receiver) => Expr::Field {
                    target: Box::new(name(&receiver)),
                    key: Key::Field(local),
                },
                None => name(&local),
            });
        }
        // A name in a constant's form is a value, so what follows its
        // `.` is a field of it and no definition of a module.
        if matches!(self.peek(), Token::Word(word) if constant_name(word))
            && matches!(self.peek_at(1), Token::Mark("."))
        {
            return Ok(name(&self.name()?));
        }
        let path = self.path()?;
        if self.at("!") && matches!(self.peek_at(1), Token::Mark("(")) {
            self.advance();
            self.advance();
            return Ok(Expr::Perform {
                ability: path,
                args: self.list(")", Self::expr)?,
            });
        }
        if self.eat("::") {
            self.expect("<")?;
            return Ok(Expr::Instantiate {
                function: path,
                args: self.list(">", Self::ty)?,
            });
        }
        Ok(Expr::Name(path))
    }

    /// A function from its type parameters on: what follows `fn`, or the
    /// name of a member or of a function in a block.
    pub(super) fn written(&mut self) -> Parsed<Written> {
        let type_params = self.type_params()?;
        self.expect("(")?;
        // A parameter written as a pattern is taken whole under a name
        // of the reader's, and the body is where it is taken apart.
        let mut apart = Vec::new();
        let mut bound = Vec::new();
        let params = self.list(")", |parser| {
            let name = if parser.at("(") || parser.at("{") {
                let pattern = parser.pattern(Mode::Binding, &mut bound)?;
                let whole = parser.made_up("param");
                apart.push((whole.clone(), pattern));
                whole
            } else {
                parser.name()?
            };
            let ty = if parser.eat(":") {
                Some(parser.ty()?)
            } else {
                None
            };
            Ok((name, ty))
        })?;
        let returns = if self.eat(":") {
            Some(self.given()?)
        } else {
            None
        };
        let row = self.row()?;
        let body = self.scoped(|parser| {
            parser
                .locals
                .extend(params.iter().map(|(name, _)| name.clone()));
            parser.locals.extend(bound);
            parser.body()
        })?;
        let body = apart
            .into_iter()
            .rev()
            .fold(body, |body, (whole, pattern)| Expr::Match {
                subject: Box::new(name(&whole)),
                arms: vec![Arm { pattern, body }],
            });
        Ok(Written {
            type_params,
            params,
            row,
            returns,
            body,
        })
    }

    /// A body: a block, or `=>` and an expression.
    pub(super) fn body(&mut self) -> Parsed<Expr> {
        if self.eat("=>") {
            self.expr()
        } else {
            self.block()
        }
    }

    /// A match that has a value, after its `match`: each arm is an
    /// expression.
    fn match_value(&mut self) -> Parsed<Expr> {
        let subject = self.expr()?;
        self.expect("{")?;
        let arms = self.list("}", |parser| {
            let mut binds = Vec::new();
            let pattern = parser.pattern(Mode::Testing, &mut binds)?;
            parser.expect("=>")?;
            let body = parser.scoped(|parser| {
                parser.locals.extend(binds);
                parser.expr()
            })?;
            Ok(Arm { pattern, body })
        })?;
        Ok(Expr::Match {
            subject: Box::new(subject),
            arms,
        })
    }
}

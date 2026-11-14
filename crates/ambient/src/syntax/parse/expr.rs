//! Expressions.

use super::super::lex::Token;
use super::patterns::Mode;
use super::{Parsed, Parser};
use crate::literal::Literal;
use crate::surface::{Arm, CellOp, Expr, Key, Operator, Path, Pattern, Type, TypeParam};

/// The binary operators, loosest first. Each level binds to the left,
/// but comparison, which does not chain.
const LEVELS: [&[&str]; 6] = [
    &["||"],
    &["&&"],
    &["==", "!=", "<=", ">=", "<", ">"],
    &["++"],
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
        let value = self.binary(0)?;
        let at = self.index;
        if !self.eat_word("as") {
            return Ok(value);
        }
        let ty = self.ty()?;
        self.expect_word("else")?;
        let cast = Expr::Cast {
            value: Box::new(value),
            ty,
            otherwise: Box::new(self.expr()?),
        };
        Ok(self.mark(at, cast))
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
            "++" => Some(Operator::Join),
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
            Token::Mark("(") => {
                self.advance();
                let mut members = self.list(")", Self::expr)?;
                let grouped = members.len() == 1 && !matches!(self.peek_back(), Token::Mark(","));
                Ok(if grouped {
                    members.remove(0)
                } else {
                    Expr::Tuple(members)
                })
            }
            Token::Mark("[") => {
                self.advance();
                Ok(Expr::List(self.list("]", Self::expr)?))
            }
            Token::Mark("{") => self.record(),
            Token::Mark("@") => self.operation(),
            Token::Word(word) => self.word(&word),
            _ => self.unexpected("an expression"),
        }
    }

    /// `@name(operands)`: an operation that has no operator.
    fn operation(&mut self) -> Parsed<Expr> {
        self.advance();
        let at = self.position();
        let Token::Word(word) = self.advance() else {
            return self.fail_at(at, "expected the name of an operation after `@`");
        };
        self.expect("(")?;
        if word == "ability_id" {
            let ability = self.path()?;
            self.expect(")")?;
            return Ok(Expr::AbilityId(ability));
        }
        let operands = self.list(")", Self::expr)?;
        let cell = |operation| Expr::Cell {
            operation,
            ty: None,
            args: operands.clone(),
        };
        let operator = match word.as_str() {
            "trace" if operands.is_empty() => return Ok(Expr::Trace),
            "read" => return Ok(cell(CellOp::Read)),
            "replace" => return Ok(cell(CellOp::Replace)),
            "wait" => return Ok(cell(CellOp::Wait)),
            "length" => Operator::Length,
            "get" => Operator::Get,
            "push" => Operator::Push,
            "join" => Operator::Join,
            "slice" => Operator::Slice,
            "to_int" => Operator::ToInt,
            "to_float" => Operator::ToFloat,
            "to_text" => Operator::ToText,
            "iterator" => Operator::Iterator,
            "iterate" => Operator::Iterate,
            "range" => Operator::Range,
            "step" => Operator::Step,
            _ => return self.fail_at(at, format!("`@{word}` is not an operation")),
        };
        Ok(operation(operator, operands))
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
            "try" | "budget" | "provide" | "observe" | "group" | "spawn" | "cell" => {
                self.scope(word)
            }
            // A join, unless the word names something nearer.
            "all" | "first"
                if !self.is_local(word) && matches!(self.peek_at(1), Token::Mark("(")) =>
            {
                self.scope(word)
            }
            _ => self.named(),
        }
    }

    /// A name: a local, or a path to a definition, which may be
    /// performed or given type arguments.
    fn named(&mut self) -> Parsed<Expr> {
        if matches!(self.peek(), Token::Word(word) if self.is_local(word)) {
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
        let params = self.list(")", |parser| {
            let name = parser.name()?;
            let ty = if parser.eat(":") {
                Some(parser.ty()?)
            } else {
                None
            };
            Ok((name, ty))
        })?;
        let returns = if self.eat(":") {
            Some(self.ty()?)
        } else {
            None
        };
        let row = self.row()?;
        let body = self.scoped(|parser| {
            parser
                .locals
                .extend(params.iter().map(|(name, _)| name.clone()));
            parser.body()
        })?;
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

//! Lowering: a core function becomes a [`lir::Function`].
//!
//! This is where the language's constructs are spelled out for a backend:
//! match becomes tests and branches, and every intermediate value gets a
//! local so that it has an owner.

use ambient_runtime::{Fault, SMALL_MAX, SMALL_MIN};

use crate::core::{self, Pattern, Primitive};
use crate::lir::{self, Expr, Op, Value};
use crate::literal::Literal;

/// A construct the backends cannot compile yet.
///
/// TODO: this goes away as the lowered form grows to cover the core tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported(pub &'static str);

/// Lowers a function.
///
/// # Errors
///
/// Fails if the function uses a construct that is not lowered yet.
pub fn lower(function: &core::Function) -> Result<lir::Function, Unsupported> {
    if !function.captures.is_empty() {
        return Err(Unsupported("lambdas"));
    }
    let mut lowering = Lowering {
        binders: function.params.len(),
        temporaries: 0,
    };
    let mut body = lowering.expr(&function.body)?;
    // Temporaries were numbered apart from the core tree's locals, whose
    // count was not known until now. They go after them.
    renumber(&mut body, lowering.binders);
    Ok(lir::Function {
        params: function.params.len(),
        locals: lowering.binders + lowering.temporaries,
        body,
    })
}

/// Where temporaries are numbered from while a function is being lowered.
const TEMPORARY: usize = 1 << 32;

fn renumber(expr: &mut Expr, base: usize) {
    let local = |local: &mut usize| {
        if *local >= TEMPORARY {
            *local = *local - TEMPORARY + base;
        }
    };
    let value = |value: &mut Value| {
        if let Value::Local(number) = value {
            local(number);
        }
    };
    match expr {
        Expr::Op(Op::Copy(operand)) => value(operand),
        Expr::Op(Op::Call { args, .. } | Op::Primitive { args, .. }) => {
            args.iter_mut().for_each(value);
        }
        Expr::Op(Op::Int(_) | Op::Float(_)) | Expr::Fault(_) => {}
        Expr::Let {
            local: bound,
            value: bound_value,
            body,
        } => {
            local(bound);
            renumber(bound_value, base);
            renumber(body, base);
        }
        Expr::If {
            condition,
            then,
            otherwise,
        } => {
            value(condition);
            renumber(then, base);
            renumber(otherwise, base);
        }
    }
}

struct Lowering {
    /// How many of the core tree's locals are bound so far. A core local
    /// keeps its number.
    binders: usize,
    temporaries: usize,
}

impl Lowering {
    fn bind(&mut self) -> usize {
        let local = self.binders;
        self.binders += 1;
        local
    }

    fn expr(&mut self, expr: &core::Expr) -> Result<Expr, Unsupported> {
        Ok(match expr {
            core::Expr::Null => Expr::Op(Op::Copy(Value::Null)),
            core::Expr::Literal(literal) => Expr::Op(match literal {
                Literal::Bool(value) => Op::Copy(Value::Bool(*value)),
                Literal::Int(value) => Op::Int(*value),
                Literal::Float(bits) => Op::Float(*bits),
                Literal::String(_) => return Err(Unsupported("strings")),
            }),
            core::Expr::Local(number) => Expr::Op(Op::Copy(Value::Local(*number))),
            core::Expr::Let { value, body } => {
                let value = self.expr(value)?;
                let local = self.bind();
                Expr::Let {
                    local,
                    value: Box::new(value),
                    body: Box::new(self.expr(body)?),
                }
            }
            core::Expr::Call { callee, args } => {
                let core::Expr::Reference(slot, _) = &**callee else {
                    return Err(Unsupported("calling a function value"));
                };
                let mut scope = Scope::default();
                let args = self.operands(args, &mut scope)?;
                scope.wrap(Expr::Op(Op::Call { slot: slot.0, args }))
            }
            core::Expr::Primitive { operation, args } => {
                let mut scope = Scope::default();
                let args = self.operands(args, &mut scope)?;
                scope.wrap(Expr::Op(Op::Primitive {
                    operation: *operation,
                    args,
                }))
            }
            core::Expr::Match { subject, arms } => {
                let mut scope = Scope::default();
                let subject = self.operand(subject, &mut scope)?;
                let mut chain = Expr::Fault(Fault::NoMatch);
                // Built from the last arm back, so each arm's failure is
                // the arms after it. Binders are numbered in arm order, so
                // the arms are lowered first.
                let mut lowered = Vec::new();
                for (pattern, body) in arms {
                    let test = self.test(pattern, subject)?;
                    let binds = self.binds(pattern);
                    let body = self.expr(body)?;
                    lowered.push((test, binds, body));
                }
                for (test, binds, body) in lowered.into_iter().rev() {
                    let body = binds.into_iter().rev().fold(body, |body, local| Expr::Let {
                        local,
                        value: Box::new(Expr::Op(Op::Copy(subject))),
                        body: Box::new(body),
                    });
                    chain = match test {
                        None => body,
                        Some(test) => {
                            let condition = self.temporary();
                            Expr::Let {
                                local: condition,
                                value: Box::new(test),
                                body: Box::new(Expr::If {
                                    condition: Value::Local(condition),
                                    then: Box::new(body),
                                    otherwise: Box::new(chain),
                                }),
                            }
                        }
                    };
                }
                scope.wrap(chain)
            }
            core::Expr::Captured(_) | core::Expr::Closure { .. } => {
                return Err(Unsupported("lambdas"));
            }
            core::Expr::Reference(..) => return Err(Unsupported("functions as values")),
            core::Expr::Record(_) | core::Expr::Field { .. } => {
                return Err(Unsupported("records"));
            }
            core::Expr::Tuple(_) => return Err(Unsupported("tuples")),
            core::Expr::List(_) => return Err(Unsupported("lists")),
            core::Expr::Cast { .. } => return Err(Unsupported("casts")),
        })
    }

    fn temporary(&mut self) -> usize {
        let local = TEMPORARY + self.temporaries;
        self.temporaries += 1;
        local
    }

    fn operands(
        &mut self,
        exprs: &[core::Expr],
        scope: &mut Scope,
    ) -> Result<Vec<Value>, Unsupported> {
        exprs.iter().map(|expr| self.operand(expr, scope)).collect()
    }

    /// `expr` as an operand. Anything that is not already a value is
    /// evaluated into a temporary that `scope` will own.
    fn operand(&mut self, expr: &core::Expr, scope: &mut Scope) -> Result<Value, Unsupported> {
        Ok(match expr {
            core::Expr::Null => Value::Null,
            core::Expr::Literal(Literal::Bool(value)) => Value::Bool(*value),
            core::Expr::Literal(Literal::Int(value)) if (SMALL_MIN..=SMALL_MAX).contains(value) => {
                Value::Int(*value)
            }
            core::Expr::Local(number) => Value::Local(*number),
            _ => {
                let value = self.expr(expr)?;
                let local = self.temporary();
                scope.bindings.push((local, value));
                Value::Local(local)
            }
        })
    }

    /// An expression that is true when `pattern` matches `subject`; `None`
    /// if it always does.
    fn test(&mut self, pattern: &Pattern, subject: Value) -> Result<Option<Expr>, Unsupported> {
        Ok(match pattern {
            Pattern::Wildcard => None,
            Pattern::Bind(inner) => self.test(inner, subject)?,
            Pattern::Literal(literal) => {
                let mut scope = Scope::default();
                let literal = self.operand(&core::Expr::Literal(literal.clone()), &mut scope)?;
                Some(scope.wrap(Expr::Op(Op::Primitive {
                    operation: Primitive::Equal,
                    args: vec![subject, literal],
                })))
            }
            Pattern::Record(_) => return Err(Unsupported("record patterns")),
            Pattern::Tuple(_) => return Err(Unsupported("tuple patterns")),
            Pattern::Is(_) => return Err(Unsupported("type tests")),
        })
    }

    /// The locals `pattern` binds, in binding order. Each is bound to the
    /// whole subject, which is all a pattern without structure can bind.
    fn binds(&mut self, pattern: &Pattern) -> Vec<usize> {
        match pattern {
            Pattern::Bind(inner) => {
                let mut locals = vec![self.bind()];
                locals.extend(self.binds(inner));
                locals
            }
            _ => Vec::new(),
        }
    }
}

/// Temporaries waiting to be bound around the expression that uses them.
#[derive(Default)]
struct Scope {
    bindings: Vec<(usize, Expr)>,
}

impl Scope {
    /// `body` inside a `Let` for each temporary, the first outermost.
    fn wrap(self, body: Expr) -> Expr {
        self.bindings
            .into_iter()
            .rev()
            .fold(body, |body, (local, value)| Expr::Let {
                local,
                value: Box::new(value),
                body: Box::new(body),
            })
    }
}

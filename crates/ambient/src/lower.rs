//! Lowering: a core function becomes a [`lir::Function`].
//!
//! This is where the language's constructs are spelled out for a backend:
//! match becomes tests and branches, and every intermediate value gets a
//! local so that it has an owner.

use ambient_runtime::{Fault, SMALL_MAX, SMALL_MIN};

use crate::core::{self, Open, Pattern, Primitive, Tree};
use crate::lir::{self, Constant, Expr, Op, Value};
use crate::literal::Literal;

/// A construct the backends cannot compile yet.
///
/// TODO: this goes away as the lowered form grows to cover the core tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported(pub &'static str);

/// Lowers a function definition.
///
/// # Errors
///
/// Fails if the definition is not a function, or uses a construct that is
/// not lowered yet.
pub fn lower(definition: &Open) -> Result<lir::Function, Unsupported> {
    let Tree::Function(function) = &definition.tree else {
        return Err(Unsupported("type definitions"));
    };
    let mut lowering = Lowering {
        binders: function.params.len(),
        temporaries: 0,
        constants: Vec::new(),
    };
    let mut body = lowering.expr(&function.body)?;
    // Temporaries were numbered apart from the core tree's locals, whose
    // count was not known until now. They go after them.
    renumber(&mut body, lowering.binders);
    Ok(lir::Function {
        params: function.params.len(),
        locals: lowering.binders + lowering.temporaries,
        slots: definition.slots.len(),
        constants: lowering.constants,
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
        Expr::Op(op) => match op {
            Op::Int(_) | Op::Float(_) | Op::Slot(_) => {}
            Op::Copy(operand) | Op::IsRecord(operand) => value(operand),
            Op::Call { args, .. } | Op::Primitive { args, .. } => args.iter_mut().for_each(value),
            Op::CallValue { callee, args } => {
                value(callee);
                args.iter_mut().for_each(value);
            }
            Op::Closure { captures, .. } => captures.iter_mut().for_each(value),
            Op::Sequence { members, .. } => members.iter_mut().for_each(value),
            Op::Record(fields) => fields.iter_mut().for_each(|(_, member)| value(member)),
            Op::Field { record, .. } => value(record),
            Op::Member { tuple, .. } => value(tuple),
            Op::HasField { value: subject, .. }
            | Op::IsTuple { value: subject, .. }
            | Op::Is { value: subject, .. } => {
                value(subject);
            }
        },
        Expr::Fault(_) => {}
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

/// What a pattern binds, and where in the subject each binding is found.
enum Plan {
    Nothing,
    /// Binds the subject to a local, then goes on inside it.
    Bind(usize, Box<Plan>),
    /// Goes on inside fields, each a constant.
    Record(Vec<(usize, Plan)>),
    /// Goes on inside members, each a position.
    Tuple(Vec<(usize, Plan)>),
}

struct Lowering {
    /// How many of the core tree's locals are bound so far. A core local
    /// keeps its number.
    binders: usize,
    temporaries: usize,
    constants: Vec<Constant>,
}

impl Lowering {
    fn bind(&mut self) -> usize {
        let local = self.binders;
        self.binders += 1;
        local
    }

    fn temporary(&mut self) -> usize {
        let local = TEMPORARY + self.temporaries;
        self.temporaries += 1;
        local
    }

    fn constant(&mut self, constant: Constant) -> usize {
        self.constants
            .iter()
            .position(|known| *known == constant)
            .unwrap_or_else(|| {
                self.constants.push(constant);
                self.constants.len() - 1
            })
    }

    fn field(&mut self, name: &str) -> usize {
        self.constant(Constant::Field(name.to_owned()))
    }

    fn expr(&mut self, expr: &core::Expr) -> Result<Expr, Unsupported> {
        let mut scope = Scope::default();
        let op = match expr {
            core::Expr::Null => Op::Copy(Value::Null),
            core::Expr::Literal(literal) => match literal {
                Literal::Bool(value) => Op::Copy(Value::Bool(*value)),
                Literal::Int(value) => Op::Int(*value),
                Literal::Float(bits) => Op::Float(*bits),
                Literal::String(_) => Op::Copy(self.operand(expr, &mut scope)?),
            },
            core::Expr::Local(_) | core::Expr::Captured(_) => {
                Op::Copy(self.operand(expr, &mut scope)?)
            }
            core::Expr::Reference(slot, _) => Op::Slot(slot.0),
            core::Expr::Let { value, body } => {
                let value = self.expr(value)?;
                let local = self.bind();
                return Ok(Expr::Let {
                    local,
                    value: Box::new(value),
                    body: Box::new(self.expr(body)?),
                });
            }
            core::Expr::Call { callee, args } => {
                if let core::Expr::Reference(slot, _) = &**callee {
                    let args = self.operands(args, &mut scope)?;
                    Op::Call { slot: slot.0, args }
                } else {
                    let callee = self.operand(callee, &mut scope)?;
                    let args = self.operands(args, &mut scope)?;
                    Op::CallValue { callee, args }
                }
            }
            core::Expr::Primitive { operation, args } => Op::Primitive {
                operation: *operation,
                args: self.operands(args, &mut scope)?,
            },
            core::Expr::Closure { slot, captures, .. } => Op::Closure {
                slot: slot.0,
                captures: self.operands(captures, &mut scope)?,
            },
            core::Expr::Tuple(members) => Op::Sequence {
                list: false,
                members: self.operands(members, &mut scope)?,
            },
            core::Expr::List(members) => Op::Sequence {
                list: true,
                members: self.operands(members, &mut scope)?,
            },
            core::Expr::Record(fields) => {
                let mut lowered = Vec::new();
                for (name, value) in fields {
                    let value = self.operand(value, &mut scope)?;
                    lowered.push((self.field(name), value));
                }
                Op::Record(lowered)
            }
            core::Expr::Field { target, name } => Op::Field {
                record: self.operand(target, &mut scope)?,
                key: self.field(name),
            },
            core::Expr::Match { subject, arms } => {
                let subject = self.operand(subject, &mut scope)?;
                return Ok(scope.wrap(self.matching(subject, arms)?));
            }
            core::Expr::Cast {
                value,
                ty,
                otherwise,
            } => {
                let value = self.operand(value, &mut scope)?;
                let test = self.is(value, ty)?;
                let otherwise = self.expr(otherwise)?;
                let kept = Expr::Op(Op::Copy(value));
                return Ok(scope.wrap(self.branch(test, kept, otherwise)));
            }
        };
        Ok(scope.wrap(Expr::Op(op)))
    }

    fn matching(
        &mut self,
        subject: Value,
        arms: &[(Pattern, core::Expr)],
    ) -> Result<Expr, Unsupported> {
        // Binders are numbered in arm order, so the arms are lowered in
        // order. The chain is then built from the last arm back, so that
        // each arm's failure is the arms after it.
        let mut lowered = Vec::new();
        for (pattern, body) in arms {
            let test = self.test(pattern, subject)?;
            let plan = self.plan(pattern);
            let body = self.expr(body)?;
            lowered.push((test, plan, body));
        }
        let mut chain = Expr::Fault(Fault::NoMatch);
        for (test, plan, body) in lowered.into_iter().rev() {
            let body = self.extract(plan, subject, body);
            chain = match test {
                None => body,
                Some(test) => self.branch(test, body, chain),
            };
        }
        Ok(chain)
    }

    /// `then` if `test` is true, `otherwise` if not.
    fn branch(&mut self, test: Expr, then: Expr, otherwise: Expr) -> Expr {
        let condition = self.temporary();
        Expr::Let {
            local: condition,
            value: Box::new(test),
            body: Box::new(Expr::If {
                condition: Value::Local(condition),
                then: Box::new(then),
                otherwise: Box::new(otherwise),
            }),
        }
    }

    /// An expression that is true when every test is, stopping at the
    /// first that is not.
    fn all(&mut self, tests: Vec<Expr>) -> Option<Expr> {
        let mut tests = tests.into_iter().rev();
        let last = tests.next()?;
        Some(tests.fold(last, |rest, test| {
            self.branch(test, rest, Expr::Op(Op::Copy(Value::Bool(false))))
        }))
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
            core::Expr::Literal(Literal::String(value)) => {
                Value::Constant(self.constant(Constant::String(value.clone())))
            }
            core::Expr::Local(number) => Value::Local(*number),
            core::Expr::Captured(position) => Value::Captured(*position),
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
            Pattern::Record(fields) => {
                let mut tests = vec![Expr::Op(Op::IsRecord(subject))];
                for (name, pattern) in fields {
                    let key = self.field(name);
                    tests.push(Expr::Op(Op::HasField {
                        value: subject,
                        key,
                    }));
                    let member = self.temporary();
                    if let Some(test) = self.test(pattern, Value::Local(member))? {
                        tests.push(Expr::Let {
                            local: member,
                            value: Box::new(Expr::Op(Op::Field {
                                record: subject,
                                key,
                            })),
                            body: Box::new(test),
                        });
                    }
                }
                self.all(tests)
            }
            Pattern::Tuple(patterns) => {
                let mut tests = vec![Expr::Op(Op::IsTuple {
                    value: subject,
                    length: patterns.len(),
                })];
                for (index, pattern) in patterns.iter().enumerate() {
                    let member = self.temporary();
                    if let Some(test) = self.test(pattern, Value::Local(member))? {
                        tests.push(Expr::Let {
                            local: member,
                            value: Box::new(Expr::Op(Op::Member {
                                tuple: subject,
                                index,
                            })),
                            body: Box::new(test),
                        });
                    }
                }
                self.all(tests)
            }
            Pattern::Is(ty) => Some(self.is(subject, ty)?),
        })
    }

    /// An expression that is true when `ty` describes `value`.
    fn is(&mut self, value: Value, ty: &core::Type) -> Result<Expr, Unsupported> {
        if mentions(ty, &|ty| matches!(ty, core::Type::Parameter(_))) {
            return Err(Unsupported("tests against a type parameter"));
        }
        if mentions(ty, &|ty| matches!(ty, core::Type::Function { .. })) {
            return Err(Unsupported("tests against a function type"));
        }
        let ty = self.constant(Constant::Type(ty.clone()));
        Ok(Expr::Op(Op::Is { value, ty }))
    }

    /// Numbers the locals `pattern` binds, in binding order, and records
    /// where each is found.
    fn plan(&mut self, pattern: &Pattern) -> Plan {
        match pattern {
            Pattern::Wildcard | Pattern::Literal(_) | Pattern::Is(_) => Plan::Nothing,
            Pattern::Bind(inner) => {
                let local = self.bind();
                Plan::Bind(local, Box::new(self.plan(inner)))
            }
            Pattern::Record(fields) => {
                let mut plans = Vec::new();
                for (name, pattern) in fields {
                    let plan = self.plan(pattern);
                    if !matches!(plan, Plan::Nothing) {
                        plans.push((self.field(name), plan));
                    }
                }
                if plans.is_empty() {
                    Plan::Nothing
                } else {
                    Plan::Record(plans)
                }
            }
            Pattern::Tuple(patterns) => {
                let mut plans = Vec::new();
                for (index, pattern) in patterns.iter().enumerate() {
                    let plan = self.plan(pattern);
                    if !matches!(plan, Plan::Nothing) {
                        plans.push((index, plan));
                    }
                }
                if plans.is_empty() {
                    Plan::Nothing
                } else {
                    Plan::Tuple(plans)
                }
            }
        }
    }

    /// `body` with everything `plan` binds bound, taken from `subject`,
    /// which the pattern is known to match.
    fn extract(&mut self, plan: Plan, subject: Value, body: Expr) -> Expr {
        match plan {
            Plan::Nothing => body,
            Plan::Bind(local, inner) => Expr::Let {
                local,
                value: Box::new(Expr::Op(Op::Copy(subject))),
                body: Box::new(self.extract(*inner, subject, body)),
            },
            Plan::Record(fields) => fields.into_iter().rev().fold(body, |body, (key, plan)| {
                let member = self.temporary();
                Expr::Let {
                    local: member,
                    value: Box::new(Expr::Op(Op::Field {
                        record: subject,
                        key,
                    })),
                    body: Box::new(self.extract(plan, Value::Local(member), body)),
                }
            }),
            Plan::Tuple(members) => members.into_iter().rev().fold(body, |body, (index, plan)| {
                let member = self.temporary();
                Expr::Let {
                    local: member,
                    value: Box::new(Expr::Op(Op::Member {
                        tuple: subject,
                        index,
                    })),
                    body: Box::new(self.extract(plan, Value::Local(member), body)),
                }
            }),
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

/// Whether `found` holds of `ty` or of any type inside it.
fn mentions(ty: &core::Type, found: &impl Fn(&core::Type) -> bool) -> bool {
    use core::Type;
    found(ty)
        || match ty {
            Type::Record(fields) => fields.iter().any(|(_, ty)| mentions(ty, found)),
            Type::Tuple(members) | Type::Union(members) | Type::Reference(_, members) => {
                members.iter().any(|ty| mentions(ty, found))
            }
            Type::List(member) => mentions(member, found),
            Type::Function { params, ret } => {
                params.iter().any(|ty| mentions(ty, found)) || mentions(ret, found)
            }
            _ => false,
        }
}

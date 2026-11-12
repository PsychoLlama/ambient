//! Match: a chain of tests and branches, then the bindings of the arm
//! that was taken.

use super::{Lowering, Scope, Unsupported};
use crate::core::{self, Pattern, Primitive};
use crate::lir::{Expr, Op, Value};

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

impl Lowering {
    pub(super) fn matching(
        &mut self,
        subject: Value,
        arms: &[(Pattern, core::Expr)],
        unmatched: Expr,
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
        let mut chain = unmatched;
        for (test, plan, body) in lowered.into_iter().rev() {
            let body = self.extract(plan, subject, body);
            chain = match test {
                None => body,
                Some(test) => self.branch(test, body, chain),
            };
        }
        Ok(chain)
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

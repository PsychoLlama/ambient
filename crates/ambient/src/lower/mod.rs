//! Lowering: a core function becomes a [`lir::Function`].
//!
//! This is where the language's constructs are spelled out for a backend:
//! match becomes tests and branches, and every intermediate value gets a
//! local so that it has an owner.

mod abilities;
mod meter;
mod patterns;
mod resume;
mod tasks;

use ambient_runtime::{SMALL_MAX, SMALL_MIN};

use crate::core::{self, Open, Tree};
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
///
/// With `metered`, the function charges for what it does: see
/// `spec/budgets.md`.
pub fn lower(definition: &Open, metered: bool) -> Result<lir::Function, Unsupported> {
    let function = match &definition.tree {
        Tree::Function(function) => function,
        Tree::Extern { params, .. } => return Ok(native(params.len(), definition.slots.len())),
        _ => return Err(Unsupported("definitions that are not functions")),
    };
    let mut lowering = Lowering {
        binders: function.params.len(),
        temporaries: 0,
        constants: Vec::new(),
        bindings: Value::Local(0),
        groups: Vec::new(),
        budgets: false,
        metered,
    };
    let entry = lowering.enter(function);
    let mut body = entry.wrap(lowering.expr(&function.body)?);
    if metered {
        body = lowering.meter(body);
    }
    // Temporaries were numbered apart from the core tree's locals, whose
    // count was not known until now. They go after them.
    let named = lowering.binders + 1;
    renumber(&mut body, named);
    let body = resume::resumable(body);
    Ok(lir::Function {
        params: function.params.len() + 1,
        locals: named + lowering.temporaries,
        slots: definition.slots.len(),
        constants: lowering.constants,
        budgets: lowering.budgets,
        body,
    })
}

/// What an extern compiles to: its arguments, as a tuple, handed to the
/// host with the bindings it was called with.
fn native(arity: usize, slots: usize) -> lir::Function {
    let args = arity + 1;
    let members = (1..=arity).map(Value::Local).collect();
    let call = Op::Native {
        index: 0,
        bindings: Value::Local(0),
        args: Value::Local(args),
    };
    lir::Function {
        params: arity + 1,
        locals: arity + 2,
        slots,
        constants: vec![Constant::Native],
        budgets: false,
        body: resume::resumable(Expr::Let {
            local: args,
            value: Box::new(Expr::Op(Op::Sequence {
                list: false,
                members,
            })),
            body: Box::new(Expr::Op(call)),
        }),
    }
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
        Expr::Op(op) => op.operands_mut().into_iter().for_each(value),
        Expr::Try { body, recover, .. } => {
            renumber(body, base);
            renumber(recover, base);
        }
        // The last two are made after temporaries are numbered.
        Expr::Fault(_) | Expr::Point { .. } | Expr::Resumed { .. } => {}
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
    /// is numbered one past its own number: local `0` is the bindings the
    /// function was handed.
    binders: usize,
    temporaries: usize,
    constants: Vec<Constant>,
    /// The bindings in scope where lowering is now: a record keyed by
    /// ability, or null where nothing is bound.
    bindings: Value,
    /// The groups lowering is inside of, innermost last.
    groups: Vec<Value>,
    /// Whether the function enters a budget.
    budgets: bool,
    /// Whether the function is being compiled to run inside budgets.
    metered: bool,
}

impl Lowering {
    pub(super) fn bind(&mut self) -> usize {
        self.binders += 1;
        self.binders
    }

    pub(super) fn temporary(&mut self) -> usize {
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

    pub(super) fn field(&mut self, name: &str) -> usize {
        self.constant(Constant::Field(name.to_owned()))
    }

    /// The constant a record is indexed by for `key`.
    pub(super) fn key(&mut self, key: &core::Key) -> usize {
        match key {
            core::Key::Field(name) => self.field(name),
            core::Key::Ability(slot) => self.constant(Constant::Ability(slot.0)),
        }
    }

    /// The closures an all or a first runs as tasks, as one tuple.
    fn thunks(&mut self, thunks: &[core::Expr], scope: &mut Scope) -> Result<Value, Unsupported> {
        let members = self.operands(thunks, scope)?;
        let tuple = self.temporary();
        let list = false;
        let made = Expr::Op(Op::Sequence { list, members });
        scope.bindings.push((tuple, made));
        Ok(Value::Local(tuple))
    }

    pub(super) fn expr(&mut self, expr: &core::Expr) -> Result<Expr, Unsupported> {
        let mut scope = Scope::default();
        let op = match expr {
            core::Expr::Null => Op::Copy(Value::Null),
            core::Expr::Literal(literal) => match literal {
                Literal::Bool(value) => Op::Copy(Value::Bool(*value)),
                Literal::Int(value) => Op::Int(*value),
                Literal::Float(bits) => Op::Float(*bits),
                Literal::String(_) | Literal::Tag { .. } => {
                    Op::Copy(self.operand(expr, &mut scope)?)
                }
            },
            core::Expr::Local(_) | core::Expr::Captured(_) | core::Expr::AbilityId(_) => {
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
                // A callee is handed the bindings in scope, whole.
                if let core::Expr::Reference(slot, _) = &**callee {
                    let args = self.arguments(self.bindings, args, &mut scope)?;
                    Op::Call { slot: slot.0, args }
                } else {
                    let callee = self.operand(callee, &mut scope)?;
                    let args = self.arguments(self.bindings, args, &mut scope)?;
                    Op::CallValue { callee, args }
                }
            }
            core::Expr::Primitive { operation, args } => Op::Primitive {
                operation: *operation,
                args: self.operands(args, &mut scope)?,
            },
            core::Expr::Closure {
                slot,
                captures,
                bindings,
                ..
            } => self.closure(*slot, captures, bindings, &mut scope)?,
            core::Expr::Perform {
                ability,
                args,
                returns,
            } => self.perform(*ability, args, *returns, &mut scope)?,
            core::Expr::Cleanup { .. }
            | core::Expr::Group(_)
            | core::Expr::Budget { .. }
            | core::Expr::Boundary(_) => return self.block(expr),
            core::Expr::Spawn(thunk) => self.spawn(thunk, &mut scope)?,
            core::Expr::All(thunks) => Op::All(self.thunks(thunks, &mut scope)?),
            core::Expr::First(thunks) => Op::First(self.thunks(thunks, &mut scope)?),
            core::Expr::Provide {
                handlers,
                only,
                body,
            } => return self.provide(handlers, *only, body),
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
                for (key, value) in fields {
                    let value = self.operand(value, &mut scope)?;
                    lowered.push((self.key(key), value));
                }
                Op::Record(lowered)
            }
            core::Expr::Field { target, key } => Op::Field {
                record: self.operand(target, &mut scope)?,
                key: self.key(key),
            },
            core::Expr::Merge { left, right, keys } => self.merge(left, right, keys, &mut scope)?,
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

    /// `then` if `test` is true, `otherwise` if not.
    pub(super) fn branch(&mut self, test: Expr, then: Expr, otherwise: Expr) -> Expr {
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
    pub(super) fn all(&mut self, tests: Vec<Expr>) -> Option<Expr> {
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
    pub(super) fn operand(
        &mut self,
        expr: &core::Expr,
        scope: &mut Scope,
    ) -> Result<Value, Unsupported> {
        Ok(match expr {
            core::Expr::Null => Value::Null,
            core::Expr::Literal(Literal::Bool(value)) => Value::Bool(*value),
            core::Expr::Literal(Literal::Int(value)) if (SMALL_MIN..=SMALL_MAX).contains(value) => {
                Value::Int(*value)
            }
            core::Expr::Literal(Literal::String(value)) => {
                Value::Constant(self.constant(Constant::String(value.clone())))
            }
            core::Expr::Literal(Literal::Tag { package, tag }) => {
                let text = Literal::tag_text(package, tag);
                Value::Constant(self.constant(Constant::Tag(text)))
            }
            core::Expr::Local(number) => Value::Local(*number + 1),
            core::Expr::Captured(position) => Value::Captured(*position),
            // The key bindings hold the ability under is its id.
            core::Expr::AbilityId(slot) => {
                Value::Constant(self.constant(Constant::Ability(slot.0)))
            }
            _ => {
                let value = self.expr(expr)?;
                let local = self.temporary();
                scope.bindings.push((local, value));
                Value::Local(local)
            }
        })
    }

    /// An expression that is true when `ty` describes `value`.
    pub(super) fn is(&mut self, value: Value, ty: &core::Type) -> Result<Expr, Unsupported> {
        if mentions(ty, &|ty| matches!(ty, core::Type::Parameter(_))) {
            return Err(Unsupported("tests against a type parameter"));
        }
        if mentions(ty, &|ty| matches!(ty, core::Type::Function { .. })) {
            return Err(Unsupported("tests against a function type"));
        }
        if mentions(ty, &|ty| matches!(ty, core::Type::Cell(_))) {
            return Err(Unsupported("tests against a cell type"));
        }
        let ty = self.constant(Constant::Type(ty.clone()));
        Ok(Expr::Op(Op::Is { value, ty }))
    }
}

/// Temporaries waiting to be bound around the expression that uses them.
#[derive(Default)]
struct Scope {
    pub(super) bindings: Vec<(usize, Expr)>,
}

impl Scope {
    /// `body` inside a `Let` for each temporary, the first outermost.
    pub(super) fn wrap(self, body: Expr) -> Expr {
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
            Type::List(member) | Type::Cell(member) => mentions(member, found),
            Type::Function { params, ret, .. } => {
                params.iter().any(|ty| mentions(ty, found)) || mentions(ret, found)
            }
            _ => false,
        }
}

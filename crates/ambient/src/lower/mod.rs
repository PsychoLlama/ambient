//! Lowering: a core function becomes a [`lir::Function`].
//!
//! This is where the language's constructs are spelled out for a backend:
//! match becomes tests and branches, and every intermediate value gets a
//! local so that it has an owner.

mod abilities;
mod meter;
mod native;
mod patterns;
mod resume;
mod tail;
mod tasks;
mod types;
mod values;

use std::collections::HashMap;
use std::ptr;

use ambient_runtime::{Fault, SMALL_MAX, SMALL_MIN};

use crate::core::{self, Open, Tree};
use crate::lir::{self, Constant, Expr, Op, Value};
use crate::literal::Literal;

pub use ambient_host::deep;
pub use types::Form;

/// A tree that lowering has no form for: one the checker never makes,
/// and the core checker refuses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported(pub &'static str);

/// Lowers a function definition.
///
/// # Errors
///
/// Fails if the definition is not a function, or uses a construct that is
/// not lowered yet.
///
/// `form` says what the tree does not: see [`Form`].
pub fn lower(definition: &Open, form: &Form) -> Result<lir::Function, Unsupported> {
    let function = match &definition.tree {
        Tree::Function(function) => function,
        Tree::Extern {
            params, suspends, ..
        } => {
            return Ok(native::native(
                params.len(),
                definition.slots.len(),
                *suspends,
            ));
        }
        _ => return Err(Unsupported("definitions that are not functions")),
    };
    // Both forms of a function read one definition, so the constants of
    // the plain form come first in the metered one's, at the same places.
    let constants = if form.metered {
        lower(definition, &form.plain())?.constants
    } else {
        Vec::new()
    };
    let positions = function.body.positions();
    let mut lowering = Lowering {
        positions: (0..)
            .zip(positions)
            .map(|(at, expr)| (ptr::from_ref(expr), at))
            .collect(),
        binders: function.params.len(),
        temporaries: 0,
        constants,
        bindings: Value::Local(0),
        // They follow what it captured: values, then bindings.
        types: if form.handed {
            let before = function.captures.len() + usize::from(!function.bindings.is_empty());
            Value::Captured(before)
        } else {
            Value::Null
        },
        type_params: function.type_params.len(),
        handed: form.slots.clone(),
        groups: Vec::new(),
        metered: form.metered,
    };
    let entry = lowering.enter(function);
    let mut body = entry.wrap(lowering.expr(&function.body)?);
    // Temporaries were numbered apart from the core tree's locals, whose
    // count was not known until now. They go after them.
    let named = lowering.binders + 1;
    renumber(&mut body, named);
    let mut locals = named + lowering.temporaries;
    let params = function.params.len() + 1;
    let own = tail::Own {
        slot: form.own,
        metered: form.metered,
        params,
    };
    let (body, points) = resume::resumable(tail::tailed(body, &mut locals, &own), form.still);
    Ok(lir::Function {
        points,
        params,
        locals,
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
        Expr::Op(op) => op.operands_mut().into_iter().for_each(value),
        Expr::Try { body, recover, .. } => {
            renumber(body, base);
            renumber(recover, base);
        }
        // The last three are made after temporaries are numbered.
        Expr::Fault(_) | Expr::Point { .. } | Expr::Resumed { .. } | Expr::Loop { .. } => {}
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
    /// Where each expression of the function's body is in it: see
    /// [`core::Expr::positions`].
    positions: HashMap<*const core::Expr, usize>,
    /// How many of the core tree's locals are bound so far. A core local
    /// is numbered one past its own number: local `0` is the bindings the
    /// function was handed.
    binders: usize,
    temporaries: usize,
    constants: Vec<Constant>,
    /// The bindings in scope where lowering is now: a record keyed by
    /// ability, or null where nothing is bound.
    bindings: Value,
    /// The type arguments the function was made at, which its closure
    /// holds if it is handed them; null if it is not.
    types: Value,
    /// Whether the function in each slot is handed its type arguments.
    handed: Vec<bool>,
    /// How many type parameters the function has.
    type_params: usize,
    /// The groups lowering is inside of, innermost last.
    groups: Vec<Value>,
    /// Whether what is being lowered runs inside a budget: the whole of
    /// a function in its metered form, and the body of a budget in any.
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
        // A node is paid for before any of it happens.
        let mut scope = Scope::default();
        self.charge(expr, &mut scope);
        Ok(scope.wrap(self.node(expr)?))
    }

    fn node(&mut self, expr: &core::Expr) -> Result<Expr, Unsupported> {
        let mut scope = Scope::default();
        let op = match expr {
            // A mark is no part of a tree: the checker takes each out.
            core::Expr::At(_, marked) => return self.node(marked),
            // Only checking tells a value from the type it is seen as.
            core::Expr::Widen { value, .. } | core::Expr::Brand { value, .. } => {
                return self.expr(value);
            }
            core::Expr::Null => Op::Copy(Value::Null),
            core::Expr::Literal(literal) => match literal {
                Literal::Bool(value) => Op::Copy(Value::Bool(*value)),
                Literal::Int(value) => Op::Int(*value),
                Literal::Float(bits) => Op::Float(*bits),
                Literal::String(_) | Literal::Bytes(_) | Literal::Tag { .. } => {
                    Op::Copy(self.operand(expr, &mut scope)?)
                }
            },
            core::Expr::Local(_) | core::Expr::Captured(_) | core::Expr::AbilityId(_) => {
                Op::Copy(self.operand(expr, &mut scope)?)
            }
            core::Expr::Reference(slot, types) => self.reference(*slot, types, &mut scope),
            core::Expr::Let { value, body } => {
                let value = self.expr(value)?;
                let local = self.bind();
                return Ok(Expr::Let {
                    local,
                    value: Box::new(value),
                    body: Box::new(self.expr(body)?),
                });
            }
            core::Expr::Call { callee, args } => self.call(callee, args, &mut scope)?,
            core::Expr::Cell { ty, value } => {
                let value = self.operand(value, &mut scope)?;
                self.cell(value, ty)
            }
            core::Expr::Primitive { operation, args } => {
                let args = self.operands(args, &mut scope)?;
                self.primitive(*operation, args, &mut scope)
            }
            core::Expr::Closure {
                slot,
                type_args,
                captures,
                bindings,
            } => self.closure(*slot, type_args, captures, bindings, &mut scope)?,
            core::Expr::Perform { .. } => self.perform(expr, &mut scope)?,
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
            core::Expr::Tuple(_) | core::Expr::List(_) | core::Expr::Record(_) => {
                self.construction(expr, &mut scope)?
            }
            core::Expr::Method(closure) => Op::Method(self.operand(closure, &mut scope)?),
            core::Expr::Field { target, key } => {
                let record = self.operand(target, &mut scope)?;
                let key = self.key(key);
                self.charge_read(record, key, &mut scope);
                Op::Field { record, key }
            }
            core::Expr::Send { .. } => self.send(expr, &mut scope)?,
            core::Expr::Step(iterator) => return self.step(expr, iterator),
            core::Expr::Instantiate { function, types } => {
                self.instantiate(function, types, &mut scope)?
            }
            core::Expr::Merge { .. } => self.merge(expr, &mut scope)?,
            core::Expr::Match { subject, arms } => {
                let subject = self.operand(subject, &mut scope)?;
                let unmatched = self.traced(expr, Expr::Fault(Fault::NoMatch));
                return Ok(scope.wrap(self.matching(subject, arms, unmatched)?));
            }
            core::Expr::Cast {
                value,
                ty,
                otherwise,
            } => {
                let value = self.operand(value, &mut scope)?;
                let test = self.is(value, ty);
                let otherwise = self.expr(otherwise)?;
                let kept = Expr::Op(Op::Copy(value));
                return Ok(scope.wrap(self.branch(test, kept, otherwise)));
            }
        };
        Ok(scope.wrap(self.operation(expr, op)))
    }

    /// The operation `node` lowered to, which goes in the trace of a
    /// fault that leaves it, if one can.
    pub(super) fn operation(&mut self, node: &core::Expr, op: Op) -> Expr {
        let faults = op.faults();
        let lowered = Expr::Op(op);
        if faults {
            self.traced(node, lowered)
        } else {
            lowered
        }
    }

    /// `lowered`, which is where a fault can leave `node`, adding the
    /// node to the trace of one that does.
    fn traced(&mut self, node: &core::Expr, lowered: Expr) -> Expr {
        // A node lowering made up has no position, and no frame.
        let Some(position) = self.positions.get(&ptr::from_ref(node)).copied() else {
            return lowered;
        };
        let definition = self.constant(Constant::Own);
        let frame = Op::Trace {
            definition,
            position,
        };
        Self::attempt(lowered, Expr::Op(frame))
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

    pub(super) fn operands(
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
        if let core::Expr::Reference(slot, types) = expr
            && let Some(instance) = self.instance(*slot, types)
        {
            return Ok(instance);
        }
        Ok(match expr {
            core::Expr::Null => Value::Null,
            core::Expr::Literal(Literal::Bool(value)) => Value::Bool(*value),
            core::Expr::Literal(Literal::Int(value)) if (SMALL_MIN..=SMALL_MAX).contains(value) => {
                Value::Int(*value)
            }
            core::Expr::Literal(Literal::String(value)) => {
                Value::Constant(self.constant(Constant::String(value.clone())))
            }
            core::Expr::Literal(Literal::Bytes(value)) => {
                Value::Constant(self.constant(Constant::Bytes(value.clone())))
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

    /// A call of a function value, metered if it is made inside a budget.
    pub(super) fn call_value(&self, callee: Value, args: Vec<Value>) -> Op {
        Op::CallValue {
            callee,
            args,
            metered: self.metered,
        }
    }

    /// A call. The callee is handed the bindings in scope, whole.
    fn call(
        &mut self,
        callee: &core::Expr,
        args: &[core::Expr],
        scope: &mut Scope,
    ) -> Result<Op, Unsupported> {
        // A function that is handed its type arguments is reached
        // through a closure that holds them, like any function value.
        if let core::Expr::Reference(slot, types) = callee
            && !self.hands(*slot, types)
        {
            let args = self.arguments(self.bindings, args, scope)?;
            let (slot, metered) = (slot.0, self.metered);
            return Ok(Op::Call {
                slot,
                args,
                metered,
            });
        }
        let callee = self.operand(callee, scope)?;
        let args = self.arguments(self.bindings, args, scope)?;
        Ok(self.call_value(callee, args))
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

//! Abilities: bindings as a record, performs as calls through it, and
//! abort as unwinding to a provide. See `spec/backends.md`.

use super::{Lowering, Scope, Unsupported};
use crate::core::{self, Slot};
use crate::lir::{Constant, Expr, Op, Value};

impl Lowering {
    /// Sets the bindings a function starts with, and returns what must be
    /// evaluated first to have them.
    pub(super) fn enter(&mut self, function: &core::Function) -> Scope {
        let mut scope = Scope::default();
        if function.bindings.is_empty() {
            return scope;
        }
        // What a lambda captured comes after the values it captured.
        let captured = Value::Captured(function.captures.len());
        if function.row.is_empty() {
            self.bindings = captured;
            return scope;
        }
        // Both: what it captured, and from what it was handed, its row.
        let row = self.ability_keys(&function.row);
        let passed = self.select(Value::Local(0), &row, &mut scope);
        self.bindings = self.bound(
            Op::Merge {
                base: captured,
                overlay: passed,
            },
            &mut scope,
        );
        scope
    }

    /// `op` evaluated into a temporary that `scope` will own.
    pub(super) fn bound(&mut self, op: Op, scope: &mut Scope) -> Value {
        let local = self.temporary();
        scope.bindings.push((local, Expr::Op(op)));
        Value::Local(local)
    }

    fn ability_key(&mut self, ability: Slot) -> usize {
        self.constant(Constant::Ability(ability.0))
    }

    /// The keys bindings hold `abilities` under. Metered code keeps
    /// beside each handler the budget it was bound inside, so those keys
    /// go wherever the handlers' do.
    pub(super) fn ability_keys(&mut self, abilities: &[Slot]) -> Vec<usize> {
        let mut keys = Vec::new();
        for ability in abilities {
            keys.push(self.ability_key(*ability));
            if self.metered {
                keys.push(self.constant(Constant::AbilityBudget(ability.0)));
            }
        }
        keys
    }

    /// Calls `handler`, which answers a perform. In metered code a handler
    /// runs inside the budget it was bound inside, not the performer's:
    /// the perform cost the performer a step, whatever the handler does.
    fn answer(&mut self, ability: Slot, callee: Value, args: Vec<Value>, scope: &mut Scope) -> Op {
        let call = self.call_value(callee, args);
        if !self.metered {
            return call;
        }
        let record = self.bindings;
        let key = self.constant(Constant::AbilityBudget(ability.0));
        let bound = self.bound(Op::Field { record, key }, scope);
        let saved = self.bound(Op::BudgetSwitch(bound), scope);
        let back = |unwinding| Expr::Op(Op::BudgetBack { saved, unwinding });
        let answered = Expr::Try {
            body: Box::new(Expr::Op(call)),
            recover: Box::new(back(true)),
            recovering: (0, 0),
        };
        let result = self.temporary();
        scope.bindings.push((result, answered));
        self.bound(
            Op::BudgetBack {
                saved,
                unwinding: false,
            },
            scope,
        );
        Op::Copy(Value::Local(result))
    }

    /// A record of exactly `keys`, each a constant, taken from `source`,
    /// which has them.
    pub(super) fn select(&mut self, source: Value, keys: &[usize], scope: &mut Scope) -> Value {
        let mut fields = Vec::new();
        for key in keys {
            let (record, key) = (source, *key);
            fields.push((key, self.bound(Op::Field { record, key }, scope)));
        }
        self.bound(Op::Record(fields), scope)
    }

    pub(super) fn closure(
        &mut self,
        slot: Slot,
        captures: &[core::Expr],
        bindings: &[Slot],
        scope: &mut Scope,
    ) -> Result<Op, Unsupported> {
        let mut captures = self.operands(captures, scope)?;
        // The bindings a lambda captures follow its values, as one record
        // of exactly those.
        if !bindings.is_empty() {
            let keys = self.ability_keys(bindings);
            captures.push(self.select(self.bindings, &keys, scope));
        }
        Ok(Op::Closure {
            slot: slot.0,
            captures,
        })
    }

    pub(super) fn merge(
        &mut self,
        left: &core::Expr,
        right: &core::Expr,
        keys: &[core::Key],
        scope: &mut Scope,
    ) -> Result<Op, Unsupported> {
        let base = self.operand(left, scope)?;
        let mut overlay = self.operand(right, scope)?;
        // A record written in place holds exactly its keys. Any other is
        // cut down to the keys it is known to hold.
        if !matches!(right, core::Expr::Record(_)) {
            let keys: Vec<usize> = keys.iter().map(|key| self.key(key)).collect();
            overlay = self.select(overlay, &keys, scope);
        }
        self.charge_copies(&[base, overlay], scope);
        Ok(Op::Merge { base, overlay })
    }

    /// The arguments of a call: the bindings the callee is handed, then
    /// what was written.
    pub(super) fn arguments(
        &mut self,
        bindings: Value,
        args: &[core::Expr],
        scope: &mut Scope,
    ) -> Result<Vec<Value>, Unsupported> {
        let mut all = vec![bindings];
        for arg in args {
            all.push(self.operand(arg, scope)?);
        }
        Ok(all)
    }

    pub(super) fn perform(
        &mut self,
        ability: Slot,
        args: &[core::Expr],
        returns: bool,
        scope: &mut Scope,
    ) -> Result<Op, Unsupported> {
        let key = self.ability_key(ability);
        let record = self.bindings;
        let binding = self.bound(Op::Field { record, key }, scope);
        if returns {
            // A handler took what it needs where it was provided.
            let args = self.arguments(Value::Null, args, scope)?;
            return Ok(self.answer(ability, binding, args, scope));
        }
        let members = self.operands(args, scope)?;
        let list = false;
        let args = self.bound(Op::Sequence { list, members }, scope);
        Ok(Op::Abort {
            token: binding,
            args,
        })
    }

    pub(super) fn provide(
        &mut self,
        handlers: &[core::Handler],
        only: bool,
        body: &core::Expr,
    ) -> Result<Expr, Unsupported> {
        let mut scope = Scope::default();
        let outer = self.bindings;
        let mut fields = Vec::new();
        // The token, handler, and arity of each handler that aborts.
        let mut aborts = Vec::new();
        for handler in handlers {
            let value = self.operand(&handler.handler, &mut scope)?;
            let key = self.ability_key(handler.ability);
            if self.metered {
                let stamp = self.constant(Constant::AbilityBudget(handler.ability.0));
                fields.push((stamp, self.bound(Op::BudgetCurrent, &mut scope)));
            }
            match handler.abort {
                None => fields.push((key, value)),
                Some(arity) => {
                    let token = self.bound(Op::Token, &mut scope);
                    fields.push((key, token));
                    aborts.push((token, value, arity));
                }
            }
        }
        let overlay = self.bound(Op::Record(fields), &mut scope);
        self.bindings = if only {
            overlay
        } else {
            let base = outer;
            self.bound(Op::Merge { base, overlay }, &mut scope)
        };
        let body = self.expr(body);
        self.bindings = outer;

        let mut result = body?;
        for (token, handler, arity) in &aborts {
            result = Expr::Try {
                body: Box::new(result),
                recover: Box::new(self.recover(*token, *handler, *arity, outer)),
                // Filled in once the function's points are numbered.
                recovering: (0, 0),
            };
        }
        if !aborts.is_empty() {
            // The body is over, so its tokens are spent. Where it unwound,
            // `recover` has seen to that already.
            let kept = self.temporary();
            let mut after = Expr::Op(Op::Copy(Value::Local(kept)));
            for (token, ..) in &aborts {
                after = Expr::Let {
                    local: self.temporary(),
                    value: Box::new(Expr::Op(Op::Expire(*token))),
                    body: Box::new(after),
                };
            }
            result = Expr::Let {
                local: kept,
                value: Box::new(result),
                body: Box::new(after),
            };
        }
        Ok(scope.wrap(result))
    }

    /// What a provide does when its body unwinds: if the unwinding is an
    /// abort to `token`, the handler's result; if not, more unwinding.
    fn recover(&mut self, token: Value, handler: Value, arity: usize, outer: Value) -> Expr {
        let caught = self.temporary();
        let members: Vec<usize> = (0..arity).map(|_| self.temporary()).collect();
        // The handler runs where the provide is written, with what is
        // bound there.
        let mut args = vec![outer];
        args.extend(members.iter().map(|member| Value::Local(*member)));
        let callee = handler;
        let mut call = Expr::Op(self.call_value(callee, args));
        for (index, member) in members.iter().enumerate().rev() {
            let tuple = Value::Local(caught);
            call = Expr::Let {
                local: *member,
                value: Box::new(Expr::Op(Op::Member { tuple, index })),
                body: Box::new(call),
            };
        }
        Expr::Let {
            local: caught,
            value: Box::new(Expr::Op(Op::Caught(token))),
            body: Box::new(call),
        }
    }
}

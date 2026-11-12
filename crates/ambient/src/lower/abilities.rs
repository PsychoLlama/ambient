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
        let passed = self.select(Value::Local(0), &function.row, &mut scope);
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
    fn bound(&mut self, op: Op, scope: &mut Scope) -> Value {
        let local = self.temporary();
        scope.bindings.push((local, Expr::Op(op)));
        Value::Local(local)
    }

    fn key(&mut self, ability: Slot) -> usize {
        self.constant(Constant::Ability(ability.0))
    }

    /// A record of exactly the bindings for `abilities`, taken from
    /// `source`.
    pub(super) fn select(&mut self, source: Value, abilities: &[Slot], scope: &mut Scope) -> Value {
        let mut fields = Vec::new();
        for ability in abilities {
            let key = self.key(*ability);
            let record = source;
            fields.push((key, self.bound(Op::Field { record, key }, scope)));
        }
        self.bound(Op::Record(fields), scope)
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
        let key = self.key(ability);
        let record = self.bindings;
        let binding = self.bound(Op::Field { record, key }, scope);
        if returns {
            // A handler took what it needs where it was provided.
            let args = self.arguments(Value::Null, args, scope)?;
            return Ok(Op::CallValue {
                callee: binding,
                args,
            });
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
            let key = self.key(handler.ability);
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
        let mut call = Expr::Op(Op::CallValue { callee, args });
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

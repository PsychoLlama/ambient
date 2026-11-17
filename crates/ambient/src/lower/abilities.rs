//! Abilities: bindings as a record, performs as calls through it, and
//! abort as unwinding to a provide. See `spec/backends.md`.

use super::{Lowering, Scope, Unsupported};
use crate::core::{self, Primitive, Slot};
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
                methods: Value::Null,
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

    /// The keys bindings hold `abilities` under.
    pub(super) fn ability_keys(&mut self, abilities: &[Slot]) -> Vec<usize> {
        abilities
            .iter()
            .map(|ability| self.ability_key(*ability))
            .collect()
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
        type_args: &[core::Type],
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
        // And the type arguments of one that is handed them follow those.
        if self.hands(slot, type_args) {
            captures.push(self.type_arguments(type_args, scope));
        }
        Ok(Op::Closure {
            slot: slot.0,
            captures,
        })
    }

    /// A call of a method: its function, with the record it is held
    /// by and then the arguments. A generic method's function is
    /// instantiated first.
    pub(super) fn send(&mut self, send: &core::Expr, scope: &mut Scope) -> Result<Op, Unsupported> {
        let core::Expr::Send {
            target,
            key,
            args,
            types,
        } = send
        else {
            return Err(Unsupported("a call of a method that is not one"));
        };
        let record = self.operand(target, scope)?;
        let key = self.key(key);
        let mut function = self.bound(Op::Unbound { record, key }, scope);
        if !types.is_empty() {
            let types = self.type_arguments(types, scope);
            function = self.bound(Op::Instantiate { function, types }, scope);
        }
        let mut all = vec![self.bindings, record];
        all.extend(self.operands(args, scope)?);
        Ok(Op::CallValue {
            callee: function,
            args: all,
        })
    }

    /// A step of an iterator: a call of the function that steps it, or,
    /// if it has none, what the runtime gives for it.
    pub(super) fn step(
        &mut self,
        node: &core::Expr,
        iterator: &core::Expr,
    ) -> Result<Expr, Unsupported> {
        let mut scope = Scope::default();
        let iterator = self.operand(iterator, &mut scope)?;
        let function = self.bound(Op::Stepper(iterator), &mut scope);
        let native = Expr::Op(Op::Primitive {
            operation: Primitive::Equal,
            args: vec![function, Value::Null],
        });
        let call = Op::CallValue {
            callee: function,
            args: vec![self.bindings],
        };
        let call = self.operation(node, call);
        let step = self.branch(native, Expr::Op(Op::Advance(iterator)), call);
        Ok(scope.wrap(step))
    }

    /// A merge of two records: see [`core::Expr::Merge`].
    pub(super) fn merge(
        &mut self,
        merge: &core::Expr,
        scope: &mut Scope,
    ) -> Result<Op, Unsupported> {
        let core::Expr::Merge {
            left,
            right,
            keys,
            methods,
        } = merge
        else {
            return Err(Unsupported("a merge of what is not one"));
        };
        let base = self.operand(left, scope)?;
        let mut overlay = self.operand(right, scope)?;
        let methods: Vec<usize> = methods.iter().map(|key| self.key(key)).collect();
        // A record written in place holds exactly its keys. Any other is
        // cut down to the keys it is known to hold. A method that is to
        // receive the merged record is taken as it is held; any other
        // is read, which binds it to the record it is read from.
        if !matches!(**right, core::Expr::Record(_)) {
            let mut fields = Vec::new();
            for key in keys {
                let (record, key) = (overlay, self.key(key));
                let taken = if methods.contains(&key) {
                    Op::Held { record, key }
                } else {
                    Op::Field { record, key }
                };
                fields.push((key, self.bound(taken, scope)));
            }
            overlay = self.bound(Op::Record(fields), scope);
        }
        let methods = if methods.is_empty() {
            Value::Null
        } else {
            let members = methods.into_iter().map(Value::Constant).collect();
            self.bound(
                Op::Sequence {
                    list: false,
                    members,
                },
                scope,
            )
        };
        Ok(Op::Merge {
            base,
            overlay,
            methods,
        })
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

    /// A perform: a call of the handler bound to the ability, or, if
    /// the ability does not return, an abort to where it was bound. A
    /// handler for a generic ability is instantiated first.
    pub(super) fn perform(
        &mut self,
        perform: &core::Expr,
        scope: &mut Scope,
    ) -> Result<Op, Unsupported> {
        let core::Expr::Perform {
            ability,
            args,
            returns,
            types,
        } = perform
        else {
            return Err(Unsupported("a perform that is not one"));
        };
        let (ability, returns) = (*ability, *returns);
        let key = self.ability_key(ability);
        let record = self.bindings;
        let mut binding = self.bound(Op::Field { record, key }, scope);
        if !types.is_empty() {
            let types = self.type_arguments(types, scope);
            let function = binding;
            binding = self.bound(Op::Instantiate { function, types }, scope);
        }
        if returns {
            // A handler took what it needs where it was provided.
            let args = self.arguments(Value::Null, args, scope)?;
            let callee = binding;
            return Ok(Op::CallValue { callee, args });
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
        // The token, handler, arity, and whether it takes the trace, of
        // each handler that aborts.
        let mut aborts = Vec::new();
        for handler in handlers {
            let value = self.operand(&handler.handler, &mut scope)?;
            let key = self.ability_key(handler.ability);
            match handler.abort {
                None => fields.push((key, value)),
                Some(arity) => {
                    let traced = handler.traced;
                    let token = self.bound(Op::Token { traced }, &mut scope);
                    fields.push((key, token));
                    aborts.push((token, value, arity, traced));
                }
            }
        }
        let overlay = self.bound(Op::Record(fields), &mut scope);
        self.bindings = if only {
            overlay
        } else {
            let base = outer;
            let methods = Value::Null;
            self.bound(
                Op::Merge {
                    base,
                    overlay,
                    methods,
                },
                &mut scope,
            )
        };
        let body = self.expr(body);
        self.bindings = outer;

        let mut result = body?;
        for (token, handler, arity, traced) in &aborts {
            let caught = Caught {
                token: *token,
                handler: *handler,
                arity: *arity,
                traced: *traced,
            };
            result = Expr::Try {
                body: Box::new(result),
                recover: Box::new(self.recover(&caught, outer)),
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
    fn recover(&mut self, abort: &Caught, outer: Value) -> Expr {
        let Caught {
            token,
            handler,
            arity,
            traced,
        } = *abort;
        let caught = self.temporary();
        let members: Vec<usize> = (0..arity).map(|_| self.temporary()).collect();
        // The handler runs where the provide is written, with what is
        // bound there.
        let mut args = vec![outer];
        args.extend(members.iter().map(|member| Value::Local(*member)));
        let trace = traced.then(|| self.temporary());
        args.extend(trace.map(Value::Local));
        let callee = handler;
        let mut call = Expr::Op(Op::CallValue { callee, args });
        if let Some(trace) = trace {
            call = Expr::Let {
                local: trace,
                value: Box::new(Expr::Op(Op::Traced)),
                body: Box::new(call),
            };
        }
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

/// A handler that aborts, as its provide holds it.
struct Caught {
    token: Value,
    handler: Value,
    /// How many arguments its ability takes.
    arity: usize,
    /// Whether it takes the trace of the abort after them.
    traced: bool,
}

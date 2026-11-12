//! Cleanups and groups: what a block does on every way out of it. See
//! `spec/cleanup.md` and `spec/tasks.md`.

use super::{Lowering, Scope, Unsupported};
use crate::core;
use crate::lir::{Cleanup, Expr, Op, Value};

/// `value` evaluated into `local` for `body`.
fn bind(local: usize, value: Expr, body: Expr) -> Expr {
    Expr::Let {
        local,
        value: Box::new(value),
        body: Box::new(body),
    }
}

impl Lowering {
    /// `first` evaluated and dropped, then `second`.
    fn then(&mut self, first: Expr, second: Expr) -> Expr {
        bind(self.temporary(), first, second)
    }

    /// `body`, or, if it unwinds, `recover`.
    pub(super) fn attempt(body: Expr, recover: Expr) -> Expr {
        Expr::Try {
            body: Box::new(body),
            recover: Box::new(recover),
            // Filled in once the function's points are numbered.
            recovering: (0, 0),
        }
    }

    /// A group's body, then a wait for the tasks spawned in it. If the
    /// body unwinds, they are cancelled and waited for all the same.
    pub(super) fn group(&mut self, body: &core::Expr) -> Result<Expr, Unsupported> {
        let group = Value::Local(self.temporary());
        let close = |unwinding| Expr::Op(Op::GroupClose { group, unwinding });
        let end = |held| Expr::Op(Op::GroupEnd { group, held });

        self.groups.push(group);
        let body = self.expr(body);
        self.groups.pop();

        let held = self.temporary();
        let unwound = self.then(close(true), end(Value::Local(held)));
        let unwound = bind(held, Expr::Op(Op::Hold), unwound);

        let result = self.temporary();
        let completed = self.then(end(Value::Null), Expr::Op(Op::Copy(Value::Local(result))));
        let completed = self.then(close(false), completed);

        let body = bind(result, Self::attempt(body?, unwound), completed);
        let Value::Local(local) = group else {
            return Ok(body);
        };
        Ok(bind(local, Expr::Op(Op::Group), body))
    }

    /// Checking put every spawn inside a group.
    pub(super) fn spawn(
        &mut self,
        thunk: &core::Expr,
        scope: &mut Scope,
    ) -> Result<Op, Unsupported> {
        let thunk = self.operand(thunk, scope)?;
        let group = self.groups.last().copied().unwrap_or(Value::Null);
        Ok(Op::Spawn { group, thunk })
    }

    pub(super) fn cleanup(
        &mut self,
        cleanup: &core::Expr,
        body: &core::Expr,
    ) -> Result<Expr, Unsupported> {
        let mut scope = Scope::default();
        // The cleanup is made first, so that it exists on every way out.
        let callee = self.operand(cleanup, &mut scope)?;
        // It took what it needs where it was made.
        let args = vec![Value::Null];
        let run = || {
            Expr::Op(Op::CallValue {
                callee,
                args: args.clone(),
            })
        };
        let moment = |moment| Expr::Op(Op::Cleanup(moment));

        // The body unwound: the cleanup runs with the unwinding held, and
        // then it goes on, whatever came of the cleanup.
        let held = self.temporary();
        let ignored = Self::attempt(run(), Expr::Op(Op::Copy(Value::Null)));
        let rethrow = Expr::Op(Op::Rethrow(Value::Local(held)));
        let unwound = bind(held, Expr::Op(Op::Hold), self.then(ignored, rethrow));

        // The body completed: the cleanup runs, and if it unwinds, so
        // does the block.
        let result = self.temporary();
        let guarded = Self::attempt(run(), moment(Cleanup::Failed));
        let after = self.then(
            moment(Cleanup::Leave),
            Expr::Op(Op::Copy(Value::Local(result))),
        );
        let after = self.then(guarded, after);
        let completed = self.then(moment(Cleanup::Enter), after);

        let body = Self::attempt(self.expr(body)?, unwound);
        Ok(scope.wrap(bind(result, body, completed)))
    }
}

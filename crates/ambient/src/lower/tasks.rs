//! Cleanups: a closure called on every way out of a block. See
//! `spec/cleanup.md`.

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
    fn attempt(body: Expr, recover: Expr) -> Expr {
        Expr::Try {
            body: Box::new(body),
            recover: Box::new(recover),
            // Filled in once the function's points are numbered.
            recovering: (0, 0),
        }
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

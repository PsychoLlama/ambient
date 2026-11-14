//! Tail calls: a call that is the last thing a function does is made
//! after the function has left. See "Calls" in `spec/backends.md`.
//!
//! The function unwinds with the call pending, and whatever called it
//! makes the call. So every call that is not a tail call is followed
//! by what does that, should its callee leave so.
//!
//! TODO: a call that is not a tail call takes stack in both backends,
//! and recursion deeper than the stack crashes the program: wasm traps,
//! and native code overflows. It should fault, which needs a depth the
//! context counts and every call checks.

use crate::lir::{Expr, Op, Value};

/// Makes every call in `body` either leave the function or stand ready
/// for its callee to. `locals` is how many locals the function uses,
/// and grows by what this adds.
pub(super) fn tailed(body: Expr, locals: &mut usize) -> Expr {
    rewrite(body, true, locals)
}

/// `last` is whether the value of `expr` is the function's, with
/// nothing left for the function to do once it is known.
fn rewrite(expr: Expr, last: bool, locals: &mut usize) -> Expr {
    match expr {
        Expr::Op(op @ (Op::Call { .. } | Op::CallValue { .. })) if last => leave(op, locals),
        Expr::Op(op @ (Op::Call { .. } | Op::CallValue { .. })) => Expr::Try {
            body: Box::new(Expr::Op(op)),
            recover: Box::new(Expr::Op(Op::Bounce)),
            recovering: (0, 0),
        },
        Expr::Let { local, value, body } => Expr::Let {
            local,
            value: Box::new(rewrite(*value, false, locals)),
            body: Box::new(rewrite(*body, last, locals)),
        },
        Expr::If {
            condition,
            then,
            otherwise,
        } => Expr::If {
            condition,
            then: Box::new(rewrite(*then, last, locals)),
            otherwise: Box::new(rewrite(*otherwise, last, locals)),
        },
        // A call that only notes itself in the trace of a fault leaves
        // too: a function that has left is in no trace.
        Expr::Try { body, recover, .. }
            if last
                && matches!(*recover, Expr::Op(Op::Trace { .. }))
                && matches!(*body, Expr::Op(Op::Call { .. } | Op::CallValue { .. })) =>
        {
            rewrite(*body, true, locals)
        }
        // Whatever else recovers has work to do after the call.
        Expr::Try {
            body,
            recover,
            recovering,
        } => Expr::Try {
            body: Box::new(rewrite(*body, false, locals)),
            recover: Box::new(rewrite(*recover, false, locals)),
            recovering,
        },
        other => other,
    }
}

/// `call` as leaving the function: its callee, as a value, and the
/// arguments the call is then made with.
fn leave(call: Op, locals: &mut usize) -> Expr {
    let mut bound = Vec::new();
    let mut bind = |op: Op| {
        let local = *locals;
        *locals += 1;
        bound.push((local, op));
        Value::Local(local)
    };
    let (callee, args, metered) = match call {
        Op::Call {
            slot,
            args,
            metered,
        } => (bind(Op::Slot(slot)), args, metered),
        Op::CallValue {
            callee,
            args,
            metered,
        } => (callee, args, metered),
        other => return Expr::Op(other),
    };
    let tail = Expr::Op(Op::Tail {
        callee,
        args,
        metered,
    });
    bound
        .into_iter()
        .rev()
        .fold(tail, |body, (local, op)| Expr::Let {
            local,
            value: Box::new(Expr::Op(op)),
            body: Box::new(body),
        })
}

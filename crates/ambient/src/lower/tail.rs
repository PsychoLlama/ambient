//! Tail calls: a call that is the last thing a function does is made
//! after the function has left. See "Calls" in `spec/backends.md`.
//!
//! The function unwinds with the call pending, and whatever called it
//! makes the call. So every call that is not a tail call is followed
//! by what does that, should its callee leave so.
//!
//! A call that is not a tail call takes stack, so a function that
//! makes one counts how deep it is: it asks as it starts, and says
//! before each call, which is where its callee counts from. See
//! `ambient_runtime::amb_enter`.

use ambient_runtime::FRAME;

use crate::lir::{Expr, Op, Value};

/// Makes every call in `body` either leave the function or stand ready
/// for its callee to. `locals` is how many locals the function uses,
/// and grows by what this adds.
pub(super) fn tailed(body: Expr, locals: &mut usize) -> Expr {
    let mut calls = Calls {
        locals,
        depth: None,
    };
    let body = calls.rewrite(body, true);
    let Some((depth, _)) = calls.depth else {
        return body;
    };
    Expr::Let {
        local: depth,
        value: Box::new(Expr::Op(Op::Enter {
            weight: *locals as u64 + FRAME,
        })),
        body: Box::new(body),
    }
}

/// The calls of a function being rewritten.
struct Calls<'a> {
    locals: &'a mut usize,
    /// The local that holds how deep the function is, and one for what
    /// saying so gives, which is nothing: both made at the first call
    /// that takes stack.
    depth: Option<(usize, usize)>,
}

impl Calls<'_> {
    fn local(&mut self) -> usize {
        *self.locals += 1;
        *self.locals - 1
    }

    /// `call` as a call that takes stack: made from the function's
    /// depth, and followed by what makes the call its callee left for.
    fn nested(&mut self, call: Op) -> Expr {
        let (depth, nothing) = if let Some(locals) = self.depth {
            locals
        } else {
            let locals = (self.local(), self.local());
            *self.depth.insert(locals)
        };
        let depth = Value::Local(depth);
        Expr::Let {
            local: nothing,
            value: Box::new(Expr::Op(Op::Depth(depth))),
            body: Box::new(Expr::Try {
                body: Box::new(Expr::Op(call)),
                recover: Box::new(Expr::Op(Op::Bounce(depth))),
                recovering: (0, 0),
            }),
        }
    }

    /// `last` is whether the value of `expr` is the function's, with
    /// nothing left for the function to do once it is known.
    fn rewrite(&mut self, expr: Expr, last: bool) -> Expr {
        match expr {
            Expr::Op(op @ (Op::Call { .. } | Op::CallValue { .. })) if last => {
                leave(op, self.locals)
            }
            Expr::Op(op @ (Op::Call { .. } | Op::CallValue { .. })) => self.nested(op),
            Expr::Let { local, value, body } => Expr::Let {
                local,
                value: Box::new(self.rewrite(*value, false)),
                body: Box::new(self.rewrite(*body, last)),
            },
            Expr::If {
                condition,
                then,
                otherwise,
            } => Expr::If {
                condition,
                then: Box::new(self.rewrite(*then, last)),
                otherwise: Box::new(self.rewrite(*otherwise, last)),
            },
            // A call that only notes itself in the trace of a fault
            // leaves too: a function that has left is in no trace.
            Expr::Try { body, recover, .. }
                if last
                    && matches!(*recover, Expr::Op(Op::Trace { .. }))
                    && matches!(*body, Expr::Op(Op::Call { .. } | Op::CallValue { .. })) =>
            {
                self.rewrite(*body, true)
            }
            // Whatever else recovers has work to do after the call.
            Expr::Try {
                body,
                recover,
                recovering,
            } => Expr::Try {
                body: Box::new(self.rewrite(*body, false)),
                recover: Box::new(self.rewrite(*recover, false)),
                recovering,
            },
            other => other,
        }
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

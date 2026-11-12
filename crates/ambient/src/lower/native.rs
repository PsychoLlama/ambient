//! What an extern compiles to. See `spec/externs.md`.

use super::{Lowering, resume};
use crate::lir::{self, Constant, Expr, Op, Value};

/// What an extern compiles to: its arguments, as a tuple, handed to the
/// host with the bindings it was called with.
pub(super) fn native(arity: usize, slots: usize) -> lir::Function {
    let args = arity + 1;
    let members = (1..=arity).map(Value::Local).collect();
    let call = Op::Native {
        index: 0,
        bindings: Value::Local(0),
        args: Value::Local(args),
    };
    // The extern is where native code fails, if it does.
    let frame = Op::Trace {
        definition: 1,
        position: 0,
    };
    let call = Lowering::attempt(Expr::Op(call), Expr::Op(frame));
    lir::Function {
        params: arity + 1,
        locals: arity + 2,
        slots,
        constants: vec![Constant::Native, Constant::Own],
        body: resume::resumable(Expr::Let {
            local: args,
            value: Box::new(Expr::Op(Op::Sequence {
                list: false,
                members,
            })),
            body: Box::new(call),
        }),
    }
}

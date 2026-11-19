//! What an extern compiles to. See `docs/spec/externs.md`.

use super::{Lowering, resume};
use crate::core::Slot;
use crate::lir::{self, Constant, Expr, Op, Value};

/// What an extern compiles to: its arguments, as a tuple, handed to the
/// host with the handlers bound for its `row`. One that `waits` is
/// where its task suspends until the host answers.
///
/// The handlers are a tuple in the order of the row, taken from the
/// bindings the extern was called with: native code is handed nothing
/// bound for an ability it did not declare.
pub(super) fn native(
    arity: usize,
    slots: usize,
    waits: bool,
    row: &[Slot],
    form: &super::Form,
) -> lir::Function {
    let args = arity + 1;
    let handlers = arity + 2;
    let mut constants = vec![Constant::Native, Constant::Own];
    constants.extend(row.iter().map(|slot| Constant::Ability(slot.0)));
    // Each handler is a field of the bindings.
    let fields = constants.len() - 2;
    let field = |at: usize| handlers + 1 + at;
    let call = Op::Native {
        index: 0,
        waits,
        bindings: if row.is_empty() {
            Value::Null
        } else {
            Value::Local(handlers)
        },
        args: Value::Local(args),
    };
    // The extern is where native code fails, if it does.
    let frame = Op::Trace {
        definition: 1,
        position: 0,
    };
    let mut body = Lowering::attempt(Expr::Op(call), Expr::Op(frame));
    let bind = |local, value: Op, body| Expr::Let {
        local,
        value: Box::new(Expr::Op(value)),
        body: Box::new(body),
    };
    if !row.is_empty() {
        let members = (0..fields).map(|at| Value::Local(field(at))).collect();
        let list = false;
        body = bind(handlers, Op::Sequence { list, members }, body);
        for at in (0..fields).rev() {
            let record = Value::Local(0);
            body = bind(
                field(at),
                Op::Field {
                    record,
                    key: at + 2,
                },
                body,
            );
        }
    }
    let members = (1..=arity).map(Value::Local).collect();
    let list = false;
    let body = bind(args, Op::Sequence { list, members }, body);
    let (body, points) = resume::resumable(body, form.still);
    lir::Function {
        params: arity + 1,
        locals: handlers + 1 + fields,
        slots,
        constants,
        points,
        body,
    }
}

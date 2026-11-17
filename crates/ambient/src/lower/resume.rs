//! Suspension: marking where a task may suspend, and what a function
//! that is resuming must not do again. See `spec/backends.md`.
//!
//! A task suspends by unwinding and resumes by running its functions
//! again from the outermost. A function that is resuming has its locals
//! back, so it must skip everything it had already computed and go to the
//! call it was in. Every intermediate value is a local, and conditions
//! are locals, so going there is running the body with every `Let` whose
//! value does not lead to that call skipped.

use crate::lir::Expr;

/// Numbers the points in `body` in the order they are reached, and marks
/// every `Let` with the points its value holds. Returns how many
/// points there are.
///
/// In a program where nothing suspends (`still`) no operation is a
/// point. A body with no point is left as it is: its function is never
/// resumed, so it has nothing to skip.
pub(super) fn resumable(body: Expr, still: bool) -> (Expr, usize) {
    if still || !suspends(&body) {
        return (body, 0);
    }
    let mut points = 0;
    (rewrite(body, &mut points), points)
}

/// Whether the task may suspend somewhere in `expr`.
fn suspends(expr: &Expr) -> bool {
    match expr {
        Expr::Op(op) => op.suspends(),
        Expr::Let { value, body, .. } => suspends(value) || suspends(body),
        Expr::If {
            then, otherwise, ..
        } => suspends(then) || suspends(otherwise),
        Expr::Try { body, recover, .. } => suspends(body) || suspends(recover),
        Expr::Point { .. } => true,
        Expr::Resumed { value, .. } => suspends(value),
        Expr::Loop { body, .. } => suspends(body),
        Expr::Fault(_) => false,
    }
}

fn rewrite(expr: Expr, next: &mut usize) -> Expr {
    match expr {
        Expr::Op(op) if op.suspends() => {
            let point = *next;
            *next += 1;
            Expr::Point { point, op }
        }
        Expr::Let { local, value, body } => {
            let from = *next;
            let value = Box::new(rewrite(*value, next));
            let points = (from, *next);
            Expr::Let {
                local,
                value: Box::new(Expr::Resumed {
                    points,
                    local,
                    value,
                }),
                body: Box::new(rewrite(*body, next)),
            }
        }
        Expr::If {
            condition,
            then,
            otherwise,
        } => Expr::If {
            condition,
            then: Box::new(rewrite(*then, next)),
            otherwise: Box::new(rewrite(*otherwise, next)),
        },
        Expr::Try { body, recover, .. } => {
            let body = Box::new(rewrite(*body, next));
            let from = *next;
            let recover = Box::new(rewrite(*recover, next));
            Expr::Try {
                body,
                recover,
                recovering: (from, *next),
            }
        }
        Expr::Loop { body, held } => Expr::Loop {
            body: Box::new(rewrite(*body, next)),
            held,
        },
        other => other,
    }
}

//! Tail calls, run on every backend. See "Calls" in `spec/backends.md`.
//!
//! What a trace says of them is tested in `traces.rs`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::surface::{Definition, Expr};
use ambient::value::Value;
use ambient_runtime::FAULT_PACKAGE;
use support::run;

/// More calls than any stack has room for.
const MANY: i64 = 1_000_000;

/// `fn name(n: int, sum: int): int`, which gives `sum` at zero and
/// `next` otherwise.
fn counting(name: &str, next: Expr) -> Definition {
    b::function(name)
        .param("n", ty::int())
        .param("sum", ty::int())
        .returns(ty::int())
        .body(b::match_(
            b::name("n"),
            [(pat::int(0), b::name("sum")), (pat::any(), next)],
        ))
}

/// A call of `callee` with one less to go and `n` more in the sum.
fn step(callee: Expr) -> Expr {
    let less = b::subtract(b::name("n"), b::int(1));
    b::call(callee, [less, b::add(b::name("sum"), b::name("n"))])
}

/// `fn main(): unknown`, with every function here beside it.
fn evaluates(body: Expr) -> Value {
    let through = ty::function([ty::int(), ty::int()], ty::int());
    let apply = b::function("apply")
        .param("f", through)
        .param("n", ty::int())
        .param("sum", ty::int())
        .returns(ty::int())
        .body(b::call(b::name("f"), [b::name("n"), b::name("sum")]));
    let definitions = [
        counting("total", step(b::name("total"))),
        counting("ping", step(b::name("pong"))),
        counting("pong", step(b::name("ping"))),
        counting("applied", {
            let less = b::subtract(b::name("n"), b::int(1));
            let sum = b::add(b::name("sum"), b::name("n"));
            b::call(b::name("apply"), [b::name("applied"), less, sum])
        }),
        apply,
        b::function("main").returns(ty::unknown()).body(body),
    ];
    run(definitions, &[]).unwrap()
}

fn from(name: &str, n: i64) -> Expr {
    b::call(b::name(name), [b::int(n), b::int(0)])
}

const SUM: Value = Value::Int(MANY * (MANY + 1) / 2);

#[test]
fn a_function_that_ends_by_calling_itself_takes_no_stack() {
    assert_eq!(evaluates(from("total", MANY)), SUM);
}

#[test]
fn functions_that_end_by_calling_each_other_take_no_stack() {
    assert_eq!(evaluates(from("ping", MANY)), SUM);
}

#[test]
fn a_function_that_ends_by_calling_a_function_value_takes_no_stack() {
    assert_eq!(evaluates(from("applied", MANY)), SUM);
}

#[test]
fn a_call_that_is_not_last_gets_what_its_callee_left_for() {
    // `total` leaves for itself, and the sum comes back to be added to.
    let more = b::add(from("total", 3), b::int(1));
    assert_eq!(evaluates(more), Value::Int(7));
}

#[test]
fn a_tail_call_inside_a_budget_is_charged_for() {
    let limit = Value::Tag {
        package: FAULT_PACKAGE.to_owned(),
        tag: "steps".to_owned(),
    };
    let budgeted = b::budget(b::int(1000), b::int(MANY), from("total", MANY));
    assert_eq!(evaluates(budgeted), limit);
}

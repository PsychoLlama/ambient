//! Tail calls, run on every backend. See "Calls" in `docs/spec/backends.md`.
//!
//! What a trace says of them is tested in `traces.rs`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::surface::{Definition, Expr};
use ambient::value::Value;
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

/// `fn walk(held: unknown, n: int): unknown`, which holds a new list
/// each time round and ends with `end`.
fn walk(end: Expr) -> Definition {
    let less = b::subtract(b::name("n"), b::int(1));
    let next = b::call(b::name("walk"), [b::list([b::name("n")]), less]);
    b::function("walk")
        .param("held", ty::unknown())
        .param("n", ty::int())
        .returns(ty::unknown())
        .body(b::match_(
            b::name("n"),
            [(pat::int(0), end), (pat::any(), next)],
        ))
}

/// What `walk` gives from a thousand, with `end` as what it ends with.
fn walked(end: Expr) -> Result<Value, ambient_runtime::Fault> {
    let start = b::call(b::name("walk"), [b::null(), b::int(1000)]);
    let main = b::function("main").returns(ty::unknown()).body(start);
    run([walk(end), main], &[])
}

#[test]
fn a_function_that_calls_itself_holds_only_what_it_was_last_given() {
    // Running it fails if anything it was given along the way is kept.
    let last = Value::List(vec![Value::Int(1)]);
    assert_eq!(walked(b::name("held")), Ok(last));
}

#[test]
fn a_fault_in_a_function_that_calls_itself_lets_go_of_what_it_was_given() {
    let overflows = b::add(b::int(i64::MAX), b::name("n"));
    let faults = b::add(overflows, b::int(1));
    assert_eq!(walked(faults), Err(ambient_runtime::Fault::Overflow));
}

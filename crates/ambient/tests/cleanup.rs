//! Cleanups, run on every backend. See `spec/cleanup.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, ty};
use ambient::surface::{Definition, Expr};
use ambient::value::Value;
use ambient_runtime::Fault;
use support::run;
use support::tasks::{note, then, until};

/// `ability fail(): never`
fn fail() -> Definition {
    b::ability("fail").returns(ty::never()).into()
}

/// Adds `entry` to the log.
fn notes(entry: i64) -> Expr {
    b::call(b::name("note"), [b::name("log"), b::int(entry)])
}

fn fails() -> Expr {
    b::perform("fail", [])
}

fn overflows() -> Expr {
    b::add(b::int(i64::MAX), b::int(1))
}

/// `body`, with `fail` handled by giving `value`.
fn recovering(value: i64, body: Expr) -> Expr {
    b::provide([("fail", b::lambda([], b::int(value)))], body)
}

/// Runs `body` with a log and a cell `c` that holds zero.
fn outcome(body: Expr) -> Result<Value, Fault> {
    let body = b::let_("c", b::cell(ty::int(), b::int(0)), body);
    let body = b::let_("log", b::cell(ty::int(), b::int(0)), body);
    let main = b::function("main").returns(ty::unknown()).body(body);
    run([note(), until(), fail(), main], &[])
}

/// What `body` gives, and the log once it is over.
fn logged(body: Expr) -> (Value, Value) {
    let both = b::let_(
        "result",
        body,
        b::tuple([b::name("result"), b::read(b::name("log"))]),
    );
    match outcome(both).unwrap() {
        Value::Tuple(ref members) => (members[0].clone(), members[1].clone()),
        other => panic!("expected a pair, got {other:?}"),
    }
}

#[test]
fn a_cleanup_runs_after_its_block_and_does_not_change_its_value() {
    let body = b::cleanup(notes(2), then(notes(1), b::string("done")));

    assert_eq!(
        logged(body),
        (Value::String("done".to_owned()), Value::Int(12))
    );
}

#[test]
fn cleanups_run_in_reverse_order_of_registration() {
    let body = b::cleanup(notes(3), b::cleanup(notes(2), notes(1)));

    assert_eq!(logged(body).1, Value::Int(123));
}

#[test]
fn a_cleanup_runs_when_its_block_aborts() {
    let body = recovering(-1, b::cleanup(notes(2), then(notes(1), fails())));

    assert_eq!(logged(body), (Value::Int(-1), Value::Int(12)));
}

#[test]
fn a_cleanup_runs_when_its_block_faults_and_the_fault_goes_on() {
    // The log is read by the outer cleanup, which a fault also runs.
    let inner = b::cleanup(notes(1), overflows());
    let reads = then(
        b::replace(b::name("c"), b::int(0), b::read(b::name("log"))),
        b::null(),
    );
    let body = b::cleanup(reads, inner);

    assert_eq!(outcome(body), Err(Fault::Overflow));
}

#[test]
fn a_cleanup_has_what_was_bound_where_it_was_registered() {
    let ask = b::ability("ask").returns(ty::int());
    let asks = b::call(b::name("note"), [b::name("log"), b::perform("ask", [])]);
    let body = b::provide(
        [("ask", b::lambda([], b::int(7)))],
        b::cleanup(asks, b::null()),
    );
    let body = b::let_(
        "log",
        b::cell(ty::int(), b::int(0)),
        then(body, b::read(b::name("log"))),
    );
    let main = b::function("main").returns(ty::unknown()).body(body);

    assert_eq!(run([note(), ask.into(), main], &[]), Ok(Value::Int(7)));
}

#[test]
fn a_fault_in_a_cleanup_does_not_stop_the_rest_or_the_unwinding() {
    // The inner cleanup faults while an abort is on its way out.
    let body = b::cleanup(notes(2), b::cleanup(overflows(), then(notes(1), fails())));

    assert_eq!(
        logged(recovering(-1, body)),
        (Value::Int(-1), Value::Int(12))
    );
}

#[test]
fn a_fault_in_the_cleanup_of_a_block_that_completed_unwinds() {
    assert_eq!(
        outcome(b::cleanup(overflows(), b::null())),
        Err(Fault::Overflow)
    );
}

#[test]
fn an_abort_that_leaves_a_cleanup_is_a_fault() {
    let body = recovering(-1, b::cleanup(fails(), b::null()));

    assert_eq!(outcome(body), Err(Fault::CleanupAbort));
}

#[test]
fn a_cleanup_handles_what_it_performs_inside_itself() {
    let body = b::cleanup(then(recovering(0, fails()), notes(1)), b::null());

    assert_eq!(logged(body).1, Value::Int(1));
}

#[test]
fn a_cancelled_task_runs_its_cleanups() {
    // The first task waits forever; the second ends the first.
    let waiting = b::cleanup(notes(2), then(notes(1), until_one()));
    let body = b::first([waiting, b::int(5)]);

    assert_eq!(logged(body), (Value::Int(5), Value::Int(12)));
}

#[test]
fn a_cleanup_that_waits_is_not_cancelled() {
    // The cancelled task's cleanup waits for a cell that is set only
    // once the cleanup has begun.
    let patient = then(notes(2), then(until_one(), notes(4)));
    let waits = b::wait(b::name("c"), b::int(0));
    let loser = b::cleanup(patient, then(notes(1), waits));
    let begun = b::call(b::name("until"), [b::name("log"), b::int(12)]);
    let sets = b::replace(b::name("c"), b::int(0), b::int(1));
    let other = then(begun, then(notes(3), sets));
    let body = b::all([b::first([loser, b::int(5)]), other]);

    assert_eq!(logged(body).1, Value::Int(1234));
}

fn until_one() -> Expr {
    b::call(b::name("until"), [b::name("c"), b::int(1)])
}

//! Native handlers that suspend: a task waits on the host while others
//! run. See "Native side" in `spec/externs.md` and `spec/tasks.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use ambient::backend::native::Natives;
use ambient::builder::{self as b, ty};
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use ambient_runtime::Fault;
use support::delays::{Delays, waiting};
use support::run_with;

/// `wait(<ticks>)`, which the host answers with `ticks` after that many.
fn wait(ticks: i64) -> Expr {
    b::perform("wait", [b::int(ticks)])
}

/// A program whose `main` is `body`, under a target that binds `wait`
/// to a native handler, which suspends if `suspending`.
fn program(suspending: bool, ret: Type, body: Expr) -> [Definition; 4] {
    let ability = b::ability("wait")
        .param("ticks", ty::int())
        .returns(ty::int());
    let handler = b::native_handler("wait", "wait");
    let handler = if suspending {
        handler.suspending()
    } else {
        handler
    };
    let target = b::target("target").bind("wait", "wait");
    let main = b::function("main")
        .performs(["wait"])
        .returns(ret)
        .body(body);
    [ability.into(), handler.into(), target.into(), main]
}

/// What `body` gives over a host that answers each wait when it is due.
fn outcome(ret: Type, body: Expr) -> Result<Value, Fault> {
    let natives = || waiting(&Rc::default());
    run_with(Some("target"), program(true, ret, body), natives)
}

fn ints<const N: usize>(values: [i64; N]) -> Value {
    Value::Tuple(values.into_iter().map(Value::Int).collect())
}

#[test]
fn a_perform_ends_when_the_host_answers() {
    assert_eq!(outcome(ty::int(), wait(3)), Ok(Value::Int(3)));
}

#[test]
fn a_task_that_waits_on_the_host_can_wait_again() {
    let body = b::add(wait(1), b::add(wait(2), wait(3)));

    assert_eq!(outcome(ty::int(), body), Ok(Value::Int(6)));
}

#[test]
fn tasks_wait_on_the_host_together() {
    let body = b::all([wait(5), wait(1), b::int(0)]);
    let ret = ty::tuple([ty::int(), ty::int(), ty::int()]);

    assert_eq!(outcome(ret, body), Ok(ints([5, 1, 0])));
}

#[test]
fn the_wait_answered_first_is_first() {
    let body = b::first([wait(5), wait(1)]);

    assert_eq!(outcome(ty::int(), body), Ok(Value::Int(1)));
}

#[test]
fn the_host_is_told_of_a_wait_its_task_left() {
    let body = b::first([wait(5), wait(1)]);
    let delays = Rc::new(RefCell::new(Delays::default()));

    run_with(Some("target"), program(true, ty::int(), body), || {
        waiting(&delays)
    })
    .unwrap();

    // The wait of five ticks, on each backend.
    assert_eq!(delays.borrow().forsaken, [5, 5]);
}

#[test]
fn a_cleanup_waits_on_the_host_though_its_task_was_cancelled() {
    // The first branch loses while it waits nine ticks, and its
    // cleanup then waits five: only the first of those is left.
    let body = b::first([support::cleanup(wait(5), wait(9)), wait(1)]);
    let delays = Rc::new(RefCell::new(Delays::default()));

    let gave = run_with(Some("target"), program(true, ty::int(), body), || {
        waiting(&delays)
    });

    assert_eq!(gave, Ok(Value::Int(1)));
    assert_eq!(delays.borrow().forsaken, [9, 9]);
}

#[test]
fn a_wait_that_fails_is_a_fault() {
    assert_eq!(outcome(ty::int(), wait(-1)), Err(Fault::Native));
}

#[test]
fn a_failed_wait_fails_the_tasks_started_with_it() {
    let body = b::all([wait(-1), wait(9)]);
    let ret = ty::tuple([ty::int(), ty::int()]);

    assert_eq!(outcome(ret, body), Err(Fault::Native));
}

#[test]
fn a_wait_no_host_answers_is_cancelled() {
    // Native code takes a ticket, and nothing brings answers.
    let natives = || {
        Natives::new().reaching("app", "main.wait", |running, _| {
            running.later().map(|_| Value::Null)
        })
    };

    assert_eq!(
        run_with(Some("target"), program(true, ty::int(), wait(1)), natives),
        Err(Fault::Cancelled)
    );
}

#[test]
fn only_a_suspending_handler_may_answer_later() {
    let natives = || waiting(&Rc::default());

    assert_eq!(
        run_with(Some("target"), program(false, ty::int(), wait(1)), natives),
        Err(Fault::Native)
    );
}

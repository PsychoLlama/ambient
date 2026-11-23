//! Native code that calls back into its program. See "Calling back" in
//! `docs/spec/externs.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::rc::Rc;

use ambient::backend::native::{Natives, Running};
use ambient::builder::{self as b, ty};
use ambient::surface::{Definition, Expr};
use ambient::value::Value;
use ambient_runtime::Fault;
use support::delays::waiting;
use support::run_with;

/// `apply!(<function>, <argument>)`: native code that calls the first
/// with the second and gives what it gave.
fn apply(function: Expr, argument: Expr) -> Expr {
    b::perform("apply", [function, argument])
}

/// A function of `n`, written in place.
fn of_n(body: Expr) -> Expr {
    b::lambda([("n", None)], body)
}

fn applied(running: &mut dyn Running, args: &[Value]) -> Result<Value, String> {
    running.call(&args[0], &args[1..])
}

/// The ability `apply`, which takes a function of an integer and an
/// integer to call it with, and its native handler.
fn applies() -> [Definition; 2] {
    let takes = ty::function([ty::int()], ty::int());
    let apply = b::ability("apply")
        .param("function", takes)
        .param("argument", ty::int())
        .returns(ty::int());
    [apply.into(), b::native_handler("apply", "apply").into()]
}

/// What `main` gives as `body`, beside `apply` and a `wait` that
/// suspends until its host answers, which `natives` adds to.
fn outcome_with(body: Expr, natives: impl Fn(Natives) -> Natives) -> Result<Value, Fault> {
    let wait = b::ability("wait")
        .param("ticks", ty::int())
        .returns(ty::int());
    let waits = b::native_handler("wait", "wait").suspending();
    let target = b::target("target")
        .bind("wait", "wait")
        .bind("apply", "apply");
    let main = b::function("main")
        .performs(["wait", "apply"])
        .returns(ty::int())
        .body(body);
    let [apply, applies] = applies();
    let definitions = [
        apply,
        applies,
        wait.into(),
        waits.into(),
        target.into(),
        main,
    ];
    run_with(Some("target"), definitions, || {
        natives(waiting(&Rc::default()).reaching("app", "main.apply", applied))
    })
}

/// What `main` gives as `body` in a program in which nothing suspends,
/// which is linked without what gives tasks their turns.
fn still(body: Expr) -> Result<Value, Fault> {
    let target = b::target("target").bind("apply", "apply");
    let main = b::function("main")
        .performs(["apply"])
        .returns(ty::int())
        .body(body);
    let [apply, applies] = applies();
    run_with(
        Some("target"),
        [apply, applies, target.into(), main],
        || Natives::new().reaching("app", "main.apply", applied),
    )
}

fn outcome(body: Expr) -> Result<Value, Fault> {
    outcome_with(body, |natives| natives)
}

#[test]
fn native_code_calls_a_function_it_is_handed() {
    let body = apply(of_n(b::add(b::name("n"), b::int(1))), b::int(41));

    assert_eq!(outcome(body), Ok(Value::Int(42)));
}

#[test]
fn what_native_code_calls_holds_what_it_captured() {
    let adds = of_n(b::add(b::name("n"), b::name("held")));
    let body = b::let_("held", b::int(40), apply(adds, b::int(2)));

    assert_eq!(outcome(body), Ok(Value::Int(42)));
}

#[test]
fn what_native_code_calls_reaches_a_cell_it_captured() {
    let puts = b::replace(b::name("held"), b::int(0), b::name("n"));
    let call = apply(of_n(b::let_("put", puts, b::name("n"))), b::int(5));
    let body = b::let_("called", call, b::read(b::name("held")));

    assert_eq!(
        outcome(b::let_("held", b::cell(ty::int(), b::int(0)), body)),
        Ok(Value::Int(5))
    );
}

#[test]
fn native_code_calls_back_in_a_program_in_which_nothing_suspends() {
    let body = apply(of_n(b::add(b::name("n"), b::int(1))), b::int(41));

    assert_eq!(still(body), Ok(Value::Int(42)));
}

#[test]
fn a_fault_in_what_native_code_calls_goes_on_through_it() {
    let body = apply(of_n(b::divide(b::name("n"), b::int(0))), b::int(1));

    assert_eq!(outcome(body), Err(Fault::DivideByZero));
}

#[test]
fn native_code_cannot_answer_for_a_call_that_unwound() {
    let swallows = |natives: Natives| {
        natives.reaching("app", "main.apply", |running, args| {
            Ok(applied(running, args).unwrap_or(Value::Int(0)))
        })
    };
    let body = apply(of_n(b::divide(b::name("n"), b::int(0))), b::int(1));

    assert_eq!(outcome_with(body, swallows), Err(Fault::DivideByZero));
}

#[test]
fn what_native_code_calls_may_start_tasks_and_wait_for_them() {
    let raced = b::first([b::add(b::name("n"), b::int(1)), b::int(0)]);

    assert_eq!(outcome(apply(of_n(raced), b::int(41))), Ok(Value::Int(42)));
}

#[test]
fn a_call_back_calls_native_code_that_calls_back() {
    let inner = apply(of_n(b::add(b::name("n"), b::int(1))), b::name("n"));
    let body = apply(of_n(b::add(inner, b::int(1))), b::int(40));

    assert_eq!(outcome(body), Ok(Value::Int(42)));
}

#[test]
fn waiting_on_the_world_beneath_native_code_is_a_fault() {
    let waits = of_n(b::perform("wait", [b::name("n")]));

    assert_eq!(outcome(apply(waits, b::int(1))), Err(Fault::Pinned));
}

#[test]
fn a_task_may_wait_on_the_world_again_after_native_code_it_called_faulted() {
    let waits = of_n(b::perform("wait", [b::name("n")]));
    let caught = b::boundary(apply(waits, b::int(1)));
    let body = b::let_("failed", caught, b::perform("wait", [b::int(7)]));

    assert_eq!(outcome(body), Ok(Value::Int(7)));
}

#[test]
fn tasks_beside_a_call_back_go_on_once_native_code_returns() {
    let raced = of_n(b::first([b::name("n"), b::int(0)]));
    let body = b::all([b::perform("wait", [b::int(2)]), apply(raced, b::int(5))]);
    let sum = b::match_(
        body,
        [(
            b::pat::tuple([b::pat::bind("waited"), b::pat::bind("called")]),
            b::add(b::name("waited"), b::name("called")),
        )],
    );

    assert_eq!(outcome(sum), Ok(Value::Int(7)));
}

#[test]
fn calls_nested_through_native_code_fault_at_the_limit() {
    let again = of_n(b::call(b::name("main"), []));

    assert_eq!(outcome(apply(again, b::int(0))), Err(Fault::Deep));
}

#[test]
fn native_code_is_told_of_a_value_it_cannot_call() {
    let confused = |natives: Natives| {
        natives.reaching("app", "main.apply", |running, args| {
            let refused = running.call(&args[1], &[]).unwrap_err();
            Ok(Value::Int(i64::from(
                refused == ambient::backend::native::UNCALLABLE,
            )))
        })
    };
    let body = apply(of_n(b::name("n")), b::int(1));

    assert_eq!(outcome_with(body, confused), Ok(Value::Int(1)));
}

#[test]
fn a_native_handler_takes_no_function_whose_type_has_a_row() {
    use ambient::engine::Engine;
    use ambient::error::Error;
    use ambient::store::MemoryStore;

    let wait = b::ability("wait")
        .param("ticks", ty::int())
        .returns(ty::int());
    let takes = ty::function_row([ty::int()], ["wait"], ty::int());
    let apply = b::ability("apply").param("function", takes);
    let applies = b::native_handler("apply", "apply");
    let workspace = support::program([wait.into(), apply.into(), applies.into()]);
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&support::value("main", "apply"));

    assert!(matches!(checked, Err(Error::NativeRow(_))));
}

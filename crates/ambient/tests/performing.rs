//! Native code that performs an ability of the row it declared. See
//! "Calling back" in `spec/externs.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::backend::native::{Natives, UNDECLARED_ABILITY};
use ambient::builder::{self as b, ty};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr};
use ambient::value::Value;
use ambient_runtime::Fault;
use support::run_with;

/// The ability `ask`, and the extern `relay`, which performs it with
/// what it is given and gives what it is answered.
fn world() -> [Definition; 2] {
    let ask = b::ability("ask").param("n", ty::int()).returns(ty::int());
    let relay = b::extern_function("relay")
        .param("n", ty::int())
        .returns(ty::int())
        .performs(["ask"]);
    [ask.into(), relay.into()]
}

fn relay(n: i64) -> Expr {
    b::call(b::name("relay"), [b::int(n)])
}

/// `body` with `ask` answered by `answer`, a function of `n`.
fn answering(answer: Expr, body: Expr) -> Expr {
    b::provide([("ask", b::lambda([("n", None)], answer))], body)
}

fn doubling(body: Expr) -> Expr {
    answering(b::multiply(b::name("n"), b::int(2)), body)
}

/// What `main` gives as `body`, where `relay` performs the ability of
/// module `main` named `performs`.
fn outcome_of(performs: &'static str, body: Expr) -> Result<Value, Fault> {
    let main = b::function("main").returns(ty::unknown()).body(body);
    let [ask, relay] = world();
    run_with(None, [ask, relay, main], || {
        Natives::new().reaching("app", "main.relay", move |running, args| {
            match running.perform("app", performs, args) {
                Err(refused) if refused == UNDECLARED_ABILITY => Ok(Value::Null),
                answered => answered,
            }
        })
    })
}

fn outcome(body: Expr) -> Result<Value, Fault> {
    outcome_of("main.ask", body)
}

#[test]
fn native_code_performs_an_ability_of_its_row() {
    assert_eq!(outcome(doubling(relay(21))), Ok(Value::Int(42)));
}

#[test]
fn what_native_code_performs_is_answered_by_the_nearest_handler() {
    let nearest = answering(b::add(b::name("n"), b::int(1)), relay(21));

    assert_eq!(outcome(doubling(nearest)), Ok(Value::Int(22)));
}

#[test]
fn native_code_performs_nothing_its_row_lacks() {
    assert_eq!(
        outcome_of("main.other", doubling(relay(21))),
        Ok(Value::Null)
    );
}

#[test]
fn a_fault_in_what_answers_native_code_goes_on_through_it() {
    let body = answering(b::divide(b::name("n"), b::int(0)), relay(1));

    assert_eq!(outcome(body), Err(Fault::DivideByZero));
}

#[test]
fn what_calls_an_extern_covers_its_row() {
    let main = b::function("main").returns(ty::int()).body(relay(1));
    let [ask, relay] = world();
    let workspace = support::program([ask, relay, main]);
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&support::value("main", "main"));

    assert!(matches!(checked, Err(Error::Unbound { .. })), "{checked:?}");
}

//! Native code that performs an ability of the row it declared. See
//! "Calling back" in `docs/spec/externs.md`.

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

/// The abilities `ask` and `relay`, and a native handler of `relay`
/// that performs `ask` with what it is given and gives what it is
/// answered. `answer`, of `n`, answers `ask` under the target `world`,
/// which binds both.
fn world(answer: Expr) -> [Definition; 5] {
    let ask = b::ability("ask").param("n", ty::int()).returns(ty::int());
    let relay = b::ability("relay").param("n", ty::int()).returns(ty::int());
    let relays = b::native_handler("relay", "relay").performs(["ask"]);
    let answers = b::function("answer")
        .param("n", ty::int())
        .returns(ty::int())
        .body(answer);
    let world = b::target("world")
        .bind("ask", "answer")
        .bind("relay", "relay");
    [
        ask.into(),
        relay.into(),
        relays.into(),
        answers,
        world.into(),
    ]
}

/// What `main` gives, which relays `n`, where `relay` performs the
/// ability of module `main` named `performs` and `ask` is answered by
/// `answer`.
fn outcome_of(performs: &'static str, answer: Expr, n: i64) -> Result<Value, Fault> {
    let main = b::function("main")
        .returns(ty::unknown())
        .performs(["relay"])
        .body(b::perform("relay", [b::int(n)]));
    let definitions = world(answer).into_iter().chain([main]);
    run_with(Some("world"), definitions, || {
        Natives::new().reaching("app", "main.relay", move |running, args| {
            match running.perform("app", performs, args) {
                Err(refused) if refused == UNDECLARED_ABILITY => Ok(Value::Null),
                answered => answered,
            }
        })
    })
}

fn doubled() -> Expr {
    b::multiply(b::name("n"), b::int(2))
}

#[test]
fn native_code_performs_an_ability_of_its_row() {
    assert_eq!(outcome_of("main.ask", doubled(), 21), Ok(Value::Int(42)));
}

#[test]
fn native_code_performs_nothing_its_row_lacks() {
    assert_eq!(outcome_of("main.other", doubled(), 21), Ok(Value::Null));
}

#[test]
fn a_fault_in_what_answers_native_code_goes_on_through_it() {
    let dividing = b::divide(b::name("n"), b::int(0));

    assert_eq!(
        outcome_of("main.ask", dividing, 1),
        Err(Fault::DivideByZero)
    );
}

#[test]
fn what_binds_a_native_handler_covers_its_row() {
    let ask = b::ability("ask").param("n", ty::int()).returns(ty::int());
    let relay = b::ability("relay").param("n", ty::int()).returns(ty::int());
    let relays = b::native_handler("relay", "relay").performs(["ask"]);
    let world = b::target("world").bind("relay", "relay");
    let main = b::function("main")
        .returns(ty::int())
        .performs(["relay"])
        .body(b::perform("relay", [b::int(1)]));
    let workspace = support::program([ask.into(), relay.into(), relays.into(), world.into(), main]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let program = engine.program(
        &support::value("main", "main"),
        &support::value("main", "world"),
    );
    let checked = engine.check(&program.unwrap());

    assert!(matches!(checked, Err(Error::Unbound { .. })), "{checked:?}");
}

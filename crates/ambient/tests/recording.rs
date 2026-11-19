//! Recording: an observer told of every ability a body performs. See
//! `docs/spec/handlers.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use ambient::backend::native::Natives;
use ambient::builder::{self as b, ModuleBuilder, ty};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr};
use ambient::value::Value;
use support::{run_with, value, workspace};

/// What the observer was told: each event's arguments and outcome.
type Seen = Rc<RefCell<Vec<(Value, Value)>>>;

/// The definitions every test starts from:
///
/// - `ability ask(n: int): int`, answered with `n + 1`;
/// - `ability fail(): never`;
/// - `ability log(event: unknown): null`, handled natively;
/// - the target `target`, which binds `ask` and `log`.
fn world() -> Vec<Definition> {
    let ask = b::ability("ask").param("n", ty::int()).returns(ty::int());
    let answer = b::function("answer")
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::add(b::name("n"), b::int(1)));
    let log = b::ability("log").param("event", ty::unknown());
    vec![
        ask.into(),
        answer,
        b::ability("fail").returns(ty::never()).into(),
        log.into(),
        b::native_handler("write", "log").into(),
        b::target("target")
            .bind("ask", "answer")
            .bind("log", "write")
            .into(),
    ]
}

/// An observer that logs every event.
fn logs() -> Expr {
    b::lambda([("event", None)], b::perform("log", [b::name("event")]))
}

fn asks(n: i64) -> Expr {
    b::perform("ask", [b::int(n)])
}

/// Runs `main(): [ask, log] int` with `body`, and gives its result and
/// what was logged. Every backend must log the same.
fn observed(body: Expr) -> (Value, Vec<(Value, Value)>) {
    let main = b::function("main")
        .performs(["ask", "log"])
        .returns(ty::int())
        .body(body);
    let seen = Seen::default();
    let natives = || {
        let seen = Rc::clone(&seen);
        Natives::new().with("app", "main.write", move |args| {
            let Value::Record(event) = &args[0] else {
                return Err("not an event".to_owned());
            };
            let part = |name: &str| event.get(name).cloned().unwrap_or(Value::Null);
            seen.borrow_mut().push((part("arguments"), part("outcome")));
            Ok(Value::Null)
        })
    };
    let result = run_with(Some("target"), world().into_iter().chain([main]), natives).unwrap();
    let mut seen = seen.take();
    let wasm = seen.split_off(seen.len() / 2);
    assert_eq!(seen, wasm, "the backends logged differently");
    (result, seen)
}

fn call(args: &[i64], outcome: i64) -> (Value, Value) {
    let args = args.iter().copied().map(Value::Int).collect();
    (Value::List(args), Value::Int(outcome))
}

#[test]
fn an_observer_is_told_each_perform_with_its_arguments_and_outcome() {
    let body = b::recording(logs(), b::add(asks(1), asks(10)));

    assert_eq!(
        observed(body),
        (Value::Int(13), vec![call(&[1], 2), call(&[10], 11)])
    );
}

#[test]
fn a_recording_sees_performs_made_by_what_its_body_calls() {
    // `fn twice(): [ask] int`
    let twice = b::lambda_row([], ["ask"], b::add(asks(1), asks(2)));
    let body = b::let_(
        "twice",
        twice,
        b::recording(logs(), b::call(b::name("twice"), [])),
    );

    assert_eq!(observed(body).1, vec![call(&[1], 2), call(&[2], 3)]);
}

#[test]
fn a_recording_does_not_see_a_handler_captured_before_it_began() {
    let captured = b::lambda([], asks(1));
    let body = b::let_(
        "early",
        captured,
        b::recording(logs(), b::call(b::name("early"), [])),
    );

    assert_eq!(observed(body), (Value::Int(2), vec![]));
}

#[test]
fn a_recording_does_not_see_what_its_body_provides_for_itself() {
    let inner = b::provide([("ask", b::lambda([("n", None)], b::int(0)))], asks(1));
    let body = b::recording(logs(), b::add(inner, asks(5)));

    assert_eq!(observed(body), (Value::Int(6), vec![call(&[5], 6)]));
}

#[test]
fn an_abort_is_reported_and_still_reaches_its_handler() {
    let failing = b::let_("unused", b::perform("fail", []), b::int(0));
    let body = b::provide(
        [("fail", b::lambda([], b::int(-1)))],
        b::recording(logs(), failing),
    );

    assert_eq!(
        observed(body),
        (Value::Int(-1), vec![(Value::List(vec![]), Value::Null)])
    );
}

#[test]
fn an_event_names_its_ability() {
    // The observer logs whether each event is for `ask`.
    let is_ask = b::equal(b::field(b::name("event"), "ability"), b::ability_id("ask"));
    let observer = b::lambda(
        [("event", None)],
        b::perform(
            "log",
            [b::record([("arguments", b::list([])), ("outcome", is_ask)])],
        ),
    );
    let (_, seen) = observed(b::recording(observer, asks(1)));

    assert_eq!(seen, vec![(Value::List(vec![]), Value::Bool(true))]);
}

// Checking

fn check(body: Expr) -> Result<(), Error> {
    let main = b::function("main").returns(ty::int()).body(body);
    let module = world()
        .into_iter()
        .chain([main])
        .fold(b::module(), ModuleBuilder::define);
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    Engine::new(&workspace, &store)
        .check(&value("main", "main"))
        .map(|_| ())
}

#[test]
fn an_observer_takes_an_event() {
    let observer = b::lambda([("count", Some(ty::int()))], b::null());
    let result = check(b::recording(observer, b::int(1)));

    assert!(matches!(result, Err(Error::Mismatch { .. })));
}

#[test]
fn a_recording_binds_nothing() {
    let quiet = b::lambda([("event", None)], b::null());
    let result = check(b::recording(quiet, asks(1)));

    assert!(matches!(result, Err(Error::Unbound { .. })));
}

#[test]
fn an_event_is_a_record_anyone_can_read() {
    let reads = b::lambda(
        [("event", None)],
        b::tuple([
            b::field(b::name("event"), "ability"),
            b::field(b::name("event"), "arguments"),
            b::field(b::name("event"), "outcome"),
        ]),
    );

    assert!(check(b::recording(reads, b::int(1))).is_ok());
}

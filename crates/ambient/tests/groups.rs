//! Groups and spawned tasks, run on every backend. See `spec/tasks.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, ty};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr};
use ambient::value::Value;
use ambient_runtime::Fault;
use support::tasks::{note, then, until};
use support::{run, value, workspace};

/// `ability fail(): never`
fn fail() -> Definition {
    b::ability("fail").returns(ty::never()).into()
}

/// `ability ask(): int`
fn ask() -> Definition {
    b::ability("ask").returns(ty::int()).into()
}

fn definitions(body: Expr) -> [Definition; 5] {
    let body = b::let_("c", b::cell(ty::int(), b::int(0)), body);
    let body = b::let_("log", b::cell(ty::int(), b::int(0)), body);
    let main = b::function("main").returns(ty::unknown()).body(body);
    [note(), until(), fail(), ask(), main]
}

/// Runs `body` with a log and a cell `c` that holds zero.
fn outcome(body: Expr) -> Result<Value, Fault> {
    run(definitions(body), &[])
}

/// What `body` gives, and the log once it is over.
fn logged(body: Expr) -> (Value, Value) {
    let pair = b::tuple([b::name("result"), b::read(b::name("log"))]);
    match outcome(b::let_("result", body, pair)).unwrap() {
        Value::Tuple(ref members) => (members[0].clone(), members[1].clone()),
        other => panic!("expected a pair, got {other:?}"),
    }
}

fn check(body: Expr) -> Result<(), Error> {
    let module = definitions(body)
        .into_iter()
        .fold(b::module(), b::ModuleBuilder::define);
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    Engine::new(&workspace, &store)
        .check(&value("main", "main"))
        .map(|_| ())
}

/// Adds `entry` to the log.
fn notes(entry: i64) -> Expr {
    b::call(b::name("note"), [b::name("log"), b::int(entry)])
}

/// Waits for the cell `c` to hold `value`.
fn waits_for(value: i64) -> Expr {
    b::call(b::name("until"), [b::name("c"), b::int(value)])
}

fn sets(value: i64) -> Expr {
    b::replace(b::name("c"), b::read(b::name("c")), b::int(value))
}

fn recovering(value: i64, body: Expr) -> Expr {
    b::provide([("fail", b::lambda([], b::int(value)))], body)
}

// Completing

#[test]
fn a_group_is_what_its_body_gives_once_its_tasks_are_over() {
    // The task runs after the body, and before the group is over.
    let body = b::group(then(b::spawn(notes(2)), then(notes(1), b::string("body"))));

    assert_eq!(
        logged(body),
        (Value::String("body".to_owned()), Value::Int(12))
    );
}

#[test]
fn a_spawn_is_null_and_drops_what_its_task_gives() {
    let body = b::group(b::spawn(b::string("dropped")));

    assert_eq!(outcome(body), Ok(Value::Null));
}

#[test]
fn a_group_spawns_as_many_tasks_as_its_body_asks_for() {
    let body = b::group(then(
        b::spawn(notes(1)),
        then(b::spawn(notes(2)), b::spawn(notes(3))),
    ));

    assert_eq!(logged(body).1, Value::Int(123));
}

#[test]
fn a_body_and_its_tasks_take_turns() {
    // The body waits for its task, which waits for the body.
    let task = then(waits_for(1), then(notes(2), sets(2)));
    let body = then(b::spawn(task), then(notes(1), then(sets(1), waits_for(2))));

    assert_eq!(logged(b::group(body)), (Value::Int(2), Value::Int(12)));
}

#[test]
fn a_task_uses_what_is_bound_outside_its_group() {
    let task = b::call(b::name("note"), [b::name("log"), b::perform("ask", [])]);
    let body = b::provide(
        [("ask", b::lambda([], b::int(7)))],
        b::group(b::spawn(task)),
    );

    assert_eq!(logged(body).1, Value::Int(7));
}

// Failing

#[test]
fn an_abort_in_a_task_ends_the_body_where_it_waits_and_leaves_the_group() {
    let body = then(
        b::spawn(b::perform("fail", [])),
        then(waits_for(1), notes(9)),
    );

    assert_eq!(
        logged(recovering(-1, b::group(body))),
        (Value::Int(-1), Value::Int(0))
    );
}

#[test]
fn a_fault_in_a_task_is_the_groups_fault() {
    let overflow = b::add(b::int(i64::MAX), b::int(1));
    let body = b::group(then(b::spawn(overflow), b::int(1)));

    assert_eq!(outcome(body), Err(Fault::Overflow));
}

#[test]
fn a_failing_task_cancels_the_others() {
    let waiting = b::cleanup(notes(2), then(notes(1), waits_for(1)));
    let body = then(b::spawn(waiting), b::spawn(b::perform("fail", [])));

    assert_eq!(
        logged(recovering(-1, b::group(body))),
        (Value::Int(-1), Value::Int(12))
    );
}

#[test]
fn a_body_that_aborts_cancels_its_tasks_and_waits_for_them() {
    let waiting = b::cleanup(notes(2), then(notes(1), waits_for(1)));
    // The body lets the task start, then aborts.
    let started = b::call(b::name("until"), [b::name("log"), b::int(1)]);
    let body = then(b::spawn(waiting), then(started, b::perform("fail", [])));

    assert_eq!(
        logged(recovering(-1, b::group(body))),
        (Value::Int(-1), Value::Int(12))
    );
}

// Where a spawn may be written

#[test]
fn a_spawn_is_written_in_the_body_of_a_group() {
    let outside = check(b::spawn(b::null()));
    let nested = check(b::group(b::call(b::lambda([], b::spawn(b::null())), [])));

    assert!(matches!(outside, Err(Error::SpawnOutsideGroup(_))));
    assert!(matches!(nested, Err(Error::SpawnOutsideGroup(_))));
}

#[test]
fn a_task_cannot_use_what_is_bound_inside_its_group() {
    let asks = || b::spawn(b::perform("ask", []));
    let provided = |body| b::provide([("ask", b::lambda([], b::int(1)))], body);
    let inside = check(b::group(provided(asks())));
    // Even where something outside binds it too: the inner one is what
    // the task would mean.
    let shadowed = check(provided(b::group(provided(asks()))));

    assert!(matches!(inside, Err(Error::SpawnBinding { .. })));
    assert!(matches!(shadowed, Err(Error::SpawnBinding { .. })));
    assert!(check(provided(b::group(asks()))).is_ok());
}

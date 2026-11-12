//! Tasks and cells, run on every backend. See `spec/tasks.md` and
//! `spec/cells.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use ambient_runtime::Fault;
use support::run;

/// `first`, then `second`.
fn then(first: Expr, second: Expr) -> Expr {
    b::let_("unused", first, second)
}

/// `fn until(c: Cell<int>, want: int): int`, which waits until the cell
/// holds `want` and then gives it.
fn until() -> Definition {
    let again = b::call(b::name("until"), [b::name("c"), b::name("want")]);
    let waits = then(b::wait(b::name("c"), b::name("seen")), again);
    let arms = [(pat::boolean(true), b::name("seen")), (pat::any(), waits)];
    let held = b::match_(b::equal(b::name("seen"), b::name("want")), arms);
    b::function("until")
        .param("c", ty::cell(ty::int()))
        .param("want", ty::int())
        .returns(ty::int())
        .body(b::let_("seen", b::read(b::name("c")), held))
}

/// `fn set(c: Cell<int>, value: int): null`, which tries until it has put
/// the value in.
fn set() -> Definition {
    let again = b::call(b::name("set"), [b::name("c"), b::name("value")]);
    let replaced = b::replace(b::name("c"), b::read(b::name("c")), b::name("value"));
    b::function("set")
        .param("c", ty::cell(ty::int()))
        .param("value", ty::int())
        .returns(ty::null())
        .body(b::match_(
            replaced,
            [(pat::boolean(true), b::null()), (pat::any(), again)],
        ))
}

/// `fn deep(c: Cell<int>, n: int): int`, which waits for the cell to hold
/// one under `n` calls of itself, and counts them.
fn deep() -> Definition {
    let deeper = b::call(
        b::name("deep"),
        [b::name("c"), b::subtract(b::name("n"), b::int(1))],
    );
    b::function("deep")
        .param("c", ty::cell(ty::int()))
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::match_(
            b::name("n"),
            [
                (pat::int(0), waits_for(1)),
                (pat::any(), b::add(b::int(1), deeper)),
            ],
        ))
}

/// `ability fail(reason: string): never`
fn fail() -> Definition {
    b::ability("fail", "fail")
        .param("reason", ty::string())
        .returns(ty::never())
        .into()
}

/// `ability ask(): int`
fn ask() -> Definition {
    b::ability("ask", "ask").returns(ty::int()).into()
}

fn waits_for(value: i64) -> Expr {
    b::call(b::name("until"), [b::name("c"), b::int(value)])
}

fn sets(value: i64) -> Expr {
    b::call(b::name("set"), [b::name("c"), b::int(value)])
}

/// `body` with `c`, a cell that holds zero.
fn with_cell(body: Expr) -> Expr {
    b::let_("c", b::cell(ty::int(), b::int(0)), body)
}

fn outcome(ret: Type, body: Expr) -> Result<Value, Fault> {
    let main = b::function("main").returns(ret).body(body);
    run([until(), set(), deep(), fail(), ask(), main], &[])
}

fn evaluates(body: Expr) -> Value {
    outcome(ty::unknown(), body).unwrap()
}

fn ints<const N: usize>(values: [i64; N]) -> Value {
    Value::Tuple(values.into_iter().map(Value::Int).collect())
}

// All

#[test]
fn all_is_the_tuple_of_what_its_tasks_give() {
    let body = b::all([b::int(1), b::string("two")]);
    let expected = Value::Tuple(vec![Value::Int(1), Value::String("two".to_owned())]);

    assert_eq!(evaluates(body), expected);
    assert_eq!(evaluates(b::all([])), Value::Tuple(vec![]));
}

#[test]
fn a_task_has_the_locals_and_abilities_of_where_it_is_written() {
    let tasks = b::all([b::add(b::name("x"), b::perform("ask", [])), b::name("x")]);
    let body = b::provide(
        [("ask", b::lambda([], b::int(10)))],
        b::let_("x", b::int(5), tasks),
    );

    assert_eq!(evaluates(body), ints([15, 5]));
}

#[test]
fn tasks_start_tasks_of_their_own() {
    let body = b::all([b::all([b::int(1), b::int(2)]), b::int(3)]);

    assert_eq!(
        evaluates(body),
        Value::Tuple(vec![ints([1, 2]), Value::Int(3)])
    );
}

// First

#[test]
fn first_is_what_the_first_task_to_be_over_gives() {
    // The first branch waits for the second, which is over at once.
    let body = with_cell(b::first([waits_for(1), b::string("quick")]));

    assert_eq!(evaluates(body), Value::String("quick".to_owned()));
}

#[test]
fn first_cancels_a_task_where_it_waits() {
    // The second branch would wait forever, under fifty calls.
    let waiting = b::call(b::name("deep"), [b::name("c"), b::int(50)]);
    let body = with_cell(b::first([then(waits_for(2), b::int(7)), waiting, sets(2)]));

    assert_eq!(evaluates(body), Value::Null);
}

#[test]
fn a_task_that_wakes_before_the_first_is_over_wins() {
    // The branch that sets the cell then waits, so the one it woke is over
    // before it.
    let body = with_cell(b::first([waits_for(1), then(sets(1), waits_for(2))]));

    assert_eq!(evaluates(body), Value::Int(1));
}

#[test]
fn an_abort_in_a_first_cancels_the_rest_and_reaches_its_provide() {
    let tasks = b::first([waits_for(1), b::perform("fail", [b::string("stop")])]);
    let body = b::provide(
        [("fail", b::lambda([("reason", None)], b::name("reason")))],
        with_cell(tasks),
    );

    assert_eq!(evaluates(body), Value::String("stop".to_owned()));
}

#[test]
fn a_first_of_nothing_waits_until_it_is_cancelled() {
    assert_eq!(outcome(ty::unknown(), b::first([])), Err(Fault::Cancelled));
    assert_eq!(
        evaluates(b::first([b::first([]), b::int(1)])),
        Value::Int(1)
    );
}

// Cells

#[test]
fn a_cell_takes_a_new_value_only_in_place_of_the_one_read() {
    let c = || b::name("c");
    let results = b::tuple([
        b::replace(c(), b::int(0), b::int(1)),
        b::replace(c(), b::int(0), b::int(2)),
        b::read(c()),
    ]);
    let expected = Value::Tuple(vec![Value::Bool(true), Value::Bool(false), Value::Int(1)]);

    assert_eq!(evaluates(with_cell(results)), expected);
}

#[test]
fn a_cell_holds_any_value_and_compares_it_by_what_it_is() {
    let c = || b::name("c");
    let replaced = b::replace(c(), b::string("a"), b::string("b"));
    let body = b::let_(
        "c",
        b::cell(ty::string(), b::string("a")),
        b::tuple([replaced, b::read(c())]),
    );
    let expected = Value::Tuple(vec![Value::Bool(true), Value::String("b".to_owned())]);

    assert_eq!(evaluates(body), expected);
}

// Suspension

#[test]
fn tasks_take_turns_through_a_cell() {
    // Each waits for what the other puts in.
    let first = then(waits_for(1), then(sets(2), b::int(10)));
    let second = then(sets(1), waits_for(2));

    assert_eq!(evaluates(with_cell(b::all([first, second]))), ints([10, 2]));
}

#[test]
fn a_task_resumes_under_every_call_it_suspended_in() {
    let waiting = b::call(b::name("deep"), [b::name("c"), b::int(50)]);
    let body = with_cell(b::all([waiting, then(sets(1), b::int(0))]));

    assert_eq!(evaluates(body), ints([51, 0]));
}

#[test]
fn a_task_resumes_with_its_locals_and_bindings() {
    let arms = [(
        pat::bind("got"),
        b::tuple([b::name("text"), b::name("got"), b::perform("ask", [])]),
    )];
    let waiting = b::provide(
        [("ask", b::lambda([], b::int(3)))],
        b::let_("text", b::string("kept"), b::match_(waits_for(1), arms)),
    );
    let body = with_cell(b::all([waiting, sets(1)]));
    let kept = Value::Tuple(vec![
        Value::String("kept".to_owned()),
        Value::Int(1),
        Value::Int(3),
    ]);

    assert_eq!(evaluates(body), Value::Tuple(vec![kept, Value::Null]));
}

// Ending early

#[test]
fn an_abort_in_one_task_cancels_the_others_and_reaches_its_provide() {
    let tasks = b::all([waits_for(1), b::perform("fail", [b::string("stop")])]);
    let body = b::provide(
        [("fail", b::lambda([("reason", None)], b::name("reason")))],
        with_cell(tasks),
    );

    assert_eq!(evaluates(body), Value::String("stop".to_owned()));
}

#[test]
fn a_fault_in_one_task_cancels_the_others_and_ends_the_program() {
    let overflow = b::add(b::int(i64::MAX), b::int(1));
    let body = with_cell(b::all([waits_for(1), overflow]));

    assert_eq!(outcome(ty::unknown(), body), Err(Fault::Overflow));
}

#[test]
fn a_program_whose_tasks_all_wait_is_cancelled() {
    let body = with_cell(b::all([waits_for(1)]));

    assert_eq!(outcome(ty::unknown(), body), Err(Fault::Cancelled));
}

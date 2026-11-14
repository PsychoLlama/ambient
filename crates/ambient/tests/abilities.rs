//! Abilities, handlers, and abort, run on every backend. See
//! `spec/abilities.md` and `spec/handlers.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use ambient_runtime::Fault;
use support::run;

/// `ability ask(): int`
fn ask() -> Definition {
    b::ability("ask").returns(ty::int()).into()
}

/// `ability base(): int`
fn base() -> Definition {
    b::ability("base").returns(ty::int()).into()
}

/// `ability fail(reason: string): never`
fn fail() -> Definition {
    b::ability("fail")
        .param("reason", ty::string())
        .returns(ty::never())
        .into()
}

fn asks() -> Expr {
    b::perform("ask", [])
}

fn fails(reason: &str) -> Expr {
    b::perform("fail", [b::string(reason)])
}

/// A handler that takes nothing and answers `value`.
fn answers(value: Expr) -> Expr {
    b::lambda([], value)
}

/// A handler for `fail` that turns the reason into `value`.
fn recovers(value: Expr) -> Expr {
    b::lambda([("reason", None)], value)
}

/// `body` with `ask` answered by `value`.
fn asking(value: i64, body: Expr) -> Expr {
    b::provide([("ask", answers(b::int(value)))], body)
}

/// Runs `main(): ret`, defined beside the abilities above and `extra`.
fn evaluates(ret: Type, body: Expr, extra: impl IntoIterator<Item = Definition>) -> Value {
    outcome(ret, body, extra).unwrap()
}

fn outcome(
    ret: Type,
    body: Expr,
    extra: impl IntoIterator<Item = Definition>,
) -> Result<Value, Fault> {
    let main = b::function("main").returns(ret).body(body);
    let definitions = [ask(), base(), fail(), main].into_iter().chain(extra);
    run(definitions, &[])
}

fn int(ret: Expr) -> Value {
    evaluates(ty::int(), ret, [])
}

/// `fn twice(): [ask] int`
fn twice() -> Definition {
    b::function("twice")
        .performs(["ask"])
        .returns(ty::int())
        .body(b::add(asks(), asks()))
}

// Performing

#[test]
fn a_perform_calls_the_handler_a_provide_bound() {
    assert_eq!(int(asking(42, asks())), Value::Int(42));
}

#[test]
fn a_handler_takes_the_arguments_of_the_perform() {
    let sum = b::ability("sum")
        .param("a", ty::int())
        .param("b", ty::int())
        .returns(ty::int());
    let handler = b::lambda(
        [("a", None), ("b", None)],
        b::add(b::name("a"), b::name("b")),
    );
    let body = b::provide(
        [("sum", handler)],
        b::perform("sum", [b::int(2), b::int(3)]),
    );

    assert_eq!(evaluates(ty::int(), body, [sum.into()]), Value::Int(5));
}

#[test]
fn a_function_performs_through_what_its_caller_passes() {
    let body = asking(21, b::call(b::name("twice"), []));

    assert_eq!(evaluates(ty::int(), body, [twice()]), Value::Int(42));
}

#[test]
fn the_innermost_provide_wins_and_ends_with_its_body() {
    let body = asking(1, b::add(asking(2, asks()), asks()));

    assert_eq!(int(body), Value::Int(3));
}

#[test]
fn a_handler_that_performs_its_own_ability_reaches_the_binding_it_replaced() {
    let inner = answers(b::add(asks(), b::int(1)));
    let body = asking(10, b::provide([("ask", inner)], asks()));

    assert_eq!(int(body), Value::Int(11));
}

#[test]
fn a_handler_takes_its_own_row_where_it_is_provided() {
    // `fn from_base(): [base] int`, bound as the handler for `ask`.
    let from_base = b::function("from_base")
        .performs(["base"])
        .returns(ty::int())
        .body(b::perform("base", []));
    let provide_ask = b::provide([("ask", b::name("from_base"))], asks());
    let body = b::provide([("base", answers(b::int(100)))], provide_ask);

    assert_eq!(evaluates(ty::int(), body, [from_base]), Value::Int(100));
}

// Capture

#[test]
fn a_captured_handler_travels_with_the_value() {
    // The lambda leaves the provide that bound `ask`, and still has it.
    let made = asking(7, b::lambda([], asks()));
    let body = b::let_("f", made, b::call(b::name("f"), []));

    assert_eq!(int(body), Value::Int(7));
}

#[test]
fn a_captured_handler_is_used_under_a_scope_that_rebinds_it() {
    let made = asking(1, b::lambda([], asks()));
    let body = b::let_("f", made, asking(2, b::call(b::name("f"), [])));

    assert_eq!(int(body), Value::Int(1));
}

#[test]
fn a_lambda_takes_its_row_from_its_caller_and_captures_the_rest() {
    // `f` takes `ask` where it is called and `base` where it is written.
    let f = b::lambda_row([], ["ask"], b::add(asks(), b::perform("base", [])));
    let rebound = b::provide([("base", answers(b::int(100)))], b::call(b::name("f"), []));
    let body = b::provide(
        [("base", answers(b::int(1)))],
        b::let_("f", f, asking(10, rebound)),
    );

    assert_eq!(int(body), Value::Int(11));
}

#[test]
fn a_function_that_takes_more_than_is_expected_of_it_is_bound_in_place() {
    // `fn apply(f: Fn(): int): int` knows nothing of `ask`.
    let apply = b::function("apply")
        .param("f", ty::function([], ty::int()))
        .returns(ty::int())
        .body(b::call(b::name("f"), []));
    let body = asking(21, b::call(b::name("apply"), [b::name("twice")]));

    assert_eq!(evaluates(ty::int(), body, [apply, twice()]), Value::Int(42));
}

#[test]
fn a_callback_given_to_a_generic_function_is_bound_in_place() {
    // `fn with<T>(f: Fn(): T): T`
    let with = b::function("with")
        .type_param("T")
        .param("f", ty::function([], ty::named("T")))
        .returns(ty::named("T"))
        .body(b::call(b::name("f"), []));
    let body = asking(21, b::call(b::name("with"), [b::name("twice")]));

    assert_eq!(evaluates(ty::int(), body, [with, twice()]), Value::Int(42));
}

// Sandboxing

#[test]
fn an_only_provide_runs_its_body_with_the_bundle_alone() {
    let body = asking(
        1,
        b::provide_only([("base", answers(b::int(5)))], b::perform("base", [])),
    );

    assert_eq!(int(body), Value::Int(5));
}

// Abort

/// `fn descend(n: int): [fail] int`, which fails at the bottom.
fn descend() -> Definition {
    let deeper = b::call(b::name("descend"), [b::subtract(b::name("n"), b::int(1))]);
    b::function("descend")
        .performs(["fail"])
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::match_(
            b::name("n"),
            [(pat::int(0), fails("bottom")), (pat::any(), deeper)],
        ))
}

#[test]
fn an_abort_makes_the_handlers_result_the_value_of_the_provide() {
    let body = b::provide(
        [("fail", recovers(b::int(-1)))],
        b::let_("unused", fails("no"), b::int(5)),
    );

    assert_eq!(int(body), Value::Int(-1));
}

#[test]
fn an_abort_unwinds_through_every_call_between() {
    let body = b::provide(
        [("fail", recovers(b::name("reason")))],
        b::call(b::name("descend"), [b::int(50)]),
    );
    let ret = ty::union([ty::int(), ty::string()]);

    assert_eq!(
        evaluates(ret, body, [descend()]),
        Value::String("bottom".to_owned())
    );
}

#[test]
fn a_provide_that_did_not_bind_the_handler_lets_the_abort_through() {
    // The inner provide binds `fail` again, for nobody; the lambda made
    // outside it aborts to the outer one.
    let inner = b::provide(
        [("fail", recovers(b::int(2)))],
        b::call(b::name("escape"), []),
    );
    let body = b::provide(
        [("fail", recovers(b::int(1)))],
        b::let_("escape", b::lambda([], fails("out")), inner),
    );

    assert_eq!(int(body), Value::Int(1));
}

#[test]
fn an_abort_handler_runs_with_what_is_bound_at_its_provide() {
    // `fn recover(reason: string): [ask] int`
    let recover = b::function("recover")
        .performs(["ask"])
        .param("reason", ty::string())
        .returns(ty::int())
        .body(asks());
    let failing = b::provide([("fail", b::name("recover"))], asking(2, fails("no")));
    let body = asking(1, failing);

    assert_eq!(evaluates(ty::int(), body, [recover]), Value::Int(1));
}

#[test]
fn performing_an_abort_handler_after_its_provide_is_a_fault() {
    // `fn never(): never`, so the handler adds nothing to the provide's type.
    let never = b::function("never")
        .returns(ty::never())
        .body(b::call(b::name("never"), []));
    let escaped = b::provide(
        [("fail", recovers(b::call(b::name("never"), [])))],
        b::lambda([], fails("late")),
    );
    let body = b::let_("late", escaped, b::call(b::name("late"), []));

    assert_eq!(outcome(ty::int(), body, [never]), Err(Fault::Expired));
}

#[test]
fn a_fault_passes_every_provide() {
    let overflow = b::add(b::int(i64::MAX), b::int(1));
    let body = b::provide([("fail", recovers(b::int(0)))], overflow);

    assert_eq!(outcome(ty::int(), body, []), Err(Fault::Overflow));
}

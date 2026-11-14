//! Bundles: records of handlers, as ordinary values. See
//! `spec/handlers.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;

use ambient::builder::{self as b, ty};
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use support::run;

/// `ability <name>(): int`, tagged with its name.
fn ability(name: &str) -> Definition {
    b::ability(name).returns(ty::int()).into()
}

fn answers(value: i64) -> Expr {
    b::lambda([], b::int(value))
}

fn asks() -> Expr {
    b::perform("ask", [])
}

/// The type of a bundle that handles `ask`.
fn asker() -> Type {
    ty::bundle([("ask", ty::function([], ty::int()))])
}

/// `fn only_ask(bundle: {[ask]: Fn(): int}): {[ask]: Fn(): int}`, which
/// forgets whatever else its argument holds.
fn only_ask() -> Definition {
    b::function("only_ask")
        .param("bundle", asker())
        .returns(asker())
        .body(b::name("bundle"))
}

/// Runs `main(): ret` beside `ask`, `base`, and `extra`.
fn evaluates(ret: Type, body: Expr, extra: impl IntoIterator<Item = Definition>) -> Value {
    let main = b::function("main").returns(ret).body(body);
    let definitions = [ability("ask"), ability("base"), main];
    run(definitions.into_iter().chain(extra), &[]).unwrap()
}

fn int(body: Expr) -> Value {
    evaluates(ty::int(), body, [])
}

#[test]
fn a_bundle_is_a_value_until_it_is_provided() {
    let body = b::let_(
        "handlers",
        b::bundle([("ask", answers(4))]),
        b::with(b::name("handlers"), asks()),
    );

    assert_eq!(int(body), Value::Int(4));
}

#[test]
fn a_function_can_build_a_bundle_over_its_arguments() {
    // `fn constant(n: int): {[ask]: Fn(): int}`
    let constant = b::function("constant")
        .param("n", ty::int())
        .returns(asker())
        .body(b::bundle([("ask", b::lambda([], b::name("n")))]));
    let body = b::with(b::call(b::name("constant"), [b::int(9)]), asks());

    assert_eq!(evaluates(ty::int(), body, [constant]), Value::Int(9));
}

#[test]
fn a_handler_can_be_read_out_of_a_bundle_and_called() {
    let bundle = b::bundle([("ask", answers(6))]);

    assert_eq!(int(b::call(b::handler(bundle, "ask"), [])), Value::Int(6));
}

#[test]
fn merging_decides_which_handler_wins() {
    let below = b::bundle([("ask", answers(1)), ("base", answers(10))]);
    let above = b::bundle([("ask", answers(2))]);
    let body = b::with(
        b::merge(below, above),
        b::add(asks(), b::perform("base", [])),
    );

    assert_eq!(int(body), Value::Int(12));
}

#[test]
fn merging_takes_only_what_the_right_side_is_known_to_hold() {
    // `fn only_b(r: {b: int}): {b: int}`. What it returns still has an `a`,
    // a string, which must not replace the left side's integer.
    let only_b = b::function("only_b")
        .param("r", ty::record([("b", ty::int())]))
        .returns(ty::record([("b", ty::int())]))
        .body(b::name("r"));
    let hidden = b::record([("a", b::string("hidden")), ("b", b::int(3))]);
    let left = b::record([("a", b::int(1)), ("b", b::int(2))]);
    let body = b::merge(left, b::call(b::name("only_b"), [hidden]));
    let ret = ty::record([("a", ty::int()), ("b", ty::int())]);

    let expected = BTreeMap::from([
        ("a".to_owned(), Value::Int(1)),
        ("b".to_owned(), Value::Int(3)),
    ]);
    assert_eq!(evaluates(ret, body, [only_b]), Value::Record(expected));
}

#[test]
fn a_provide_binds_only_what_the_bundles_type_lists() {
    // The bundle also handles `base`, but nothing provided says so.
    let wide = b::bundle([("ask", answers(1)), ("base", answers(100))]);
    let narrowed = b::call(b::name("only_ask"), [wide]);
    let body = b::provide(
        [("base", answers(10))],
        b::with(narrowed, b::add(asks(), b::perform("base", []))),
    );

    assert_eq!(evaluates(ty::int(), body, [only_ask()]), Value::Int(11));
}

#[test]
fn a_bundle_can_hold_a_handler_that_aborts() {
    let fail = b::ability("fail")
        .param("reason", ty::string())
        .returns(ty::never());
    let handlers = b::bundle([("fail", b::lambda([("reason", None)], b::int(-1)))]);
    let failing = b::let_("unused", b::perform("fail", [b::string("no")]), b::int(1));
    let body = b::let_("handlers", handlers, b::with(b::name("handlers"), failing));

    assert_eq!(evaluates(ty::int(), body, [fail.into()]), Value::Int(-1));
}

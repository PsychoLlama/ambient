//! Values of generic function types, run on every backend. See
//! "Generics" in `spec/types.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use support::run;

fn t() -> Type {
    ty::named("T")
}

/// `<U>(U) -> U`
fn identity() -> Type {
    ty::generic(
        [("U", None)],
        ty::function([ty::named("U")], ty::named("U")),
    )
}

/// `fn id<T>(v: T): T`
fn id() -> Definition {
    b::function("id")
        .type_param("T")
        .param("v", t())
        .returns(t())
        .body(b::name("v"))
}

/// `fn is_a<T>(v: unknown): bool`, which needs its type argument as it
/// runs.
fn is_a() -> Definition {
    let arms = [
        (pat::is(t()), b::boolean(true)),
        (pat::any(), b::boolean(false)),
    ];
    b::function("is_a")
        .type_param("T")
        .param("v", ty::unknown())
        .returns(ty::bool())
        .body(b::match_(b::name("v"), arms))
}

/// `(f(1), f("a"))`
fn at_two_types(f: &str) -> Expr {
    let call = |arg| b::call(b::name(f), [arg]);
    b::tuple([call(b::int(1)), call(b::string("a"))])
}

fn both() -> Value {
    Value::Tuple(vec![Value::Int(1), Value::String("a".into())])
}

/// What `body` evaluates to, beside `id`, `is_a`, and `extra`.
fn evaluates(body: Expr, extra: impl IntoIterator<Item = Definition>) -> Value {
    let main = b::function("main").returns(ty::unknown()).body(body);
    run([id(), is_a(), main].into_iter().chain(extra), &[]).unwrap()
}

#[test]
fn a_generic_function_is_a_value_used_at_more_than_one_type() {
    let body = b::let_("f", b::name("id"), at_two_types("f"));
    assert_eq!(evaluates(body, []), both());
}

#[test]
fn a_generic_function_is_passed_to_a_function_that_asks_for_one() {
    let twice = b::function("twice")
        .param("f", identity())
        .returns(ty::unknown())
        .body(at_two_types("f"));
    let body = b::call(b::name("twice"), [b::name("id")]);
    assert_eq!(evaluates(body, [twice]), both());
}

#[test]
fn a_generic_value_is_instantiated_where_a_function_is_expected() {
    let apply = b::function("apply")
        .param("f", ty::function([ty::int()], ty::int()))
        .returns(ty::int())
        .body(b::call(b::name("f"), [b::int(3)]));
    let body = b::let_(
        "g",
        b::name("id"),
        b::call(b::name("apply"), [b::name("g")]),
    );
    assert_eq!(evaluates(body, [apply]), Value::Int(3));
}

#[test]
fn a_generic_value_that_needs_its_type_arguments_is_handed_them() {
    let at = |member: Type, value: Expr| b::call(b::instantiate("f", [member]), [value]);
    let tests = b::tuple([
        at(ty::int(), b::int(1)),
        at(ty::int(), b::string("a")),
        at(ty::string(), b::string("a")),
    ]);
    let expected = [true, false, true].map(Value::Bool);
    let body = b::let_("f", b::name("is_a"), tests);
    assert_eq!(evaluates(body, []), Value::Tuple(expected.to_vec()));
}

#[test]
fn a_record_holds_a_generic_function_in_a_field() {
    let record = b::record([("same", b::name("id"))]);
    let sent = |arg| b::send(b::name("record"), "same", [arg]);
    let body = b::tuple([sent(b::int(1)), sent(b::string("a"))]);
    assert_eq!(evaluates(b::let_("record", record, body), []), both());
}

/// Whether `value`, its type forgotten, is of type `ty`.
fn is(ty: Type, value: Expr) -> Expr {
    let unknown = b::cast(value, ty::unknown(), b::null());
    let arms = [
        (pat::is(ty), b::boolean(true)),
        (pat::any(), b::boolean(false)),
    ];
    b::match_(unknown, arms)
}

#[test]
fn a_generic_function_comes_back_out_of_unknown_as_what_it_is() {
    let plain = ty::function([ty::int()], ty::int());
    let tests = b::tuple([
        is(identity(), b::name("id")),
        is(plain.clone(), b::name("id")),
        is(identity(), b::instantiate("is_a", [ty::int()])),
    ]);
    let expected = [true, false, false].map(Value::Bool);
    assert_eq!(evaluates(tests, []), Value::Tuple(expected.to_vec()));
}

#[test]
fn generic_functions_are_compared_as_values() {
    let same = b::equal(b::name("id"), b::name("id"));
    let other = b::equal(
        b::instantiate("is_a", [ty::int()]),
        b::instantiate("is_a", [ty::string()]),
    );
    let expected = [true, false].map(Value::Bool);
    assert_eq!(
        evaluates(b::tuple([same, other]), []),
        Value::Tuple(expected.to_vec())
    );
}

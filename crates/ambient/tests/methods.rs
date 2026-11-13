//! Methods of records, run on every backend. See `spec/methods.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use support::{run, value, workspace};

/// A record with an `x`.
fn has_x() -> Type {
    ty::record([("x", ty::int())])
}

/// A record with an `x` and a function `double`.
fn doubles() -> Type {
    ty::record([("x", ty::int()), ("double", ty::function([], ty::int()))])
}

/// `fn double(self: {x: int}): int`, twice the record's `x`.
fn double() -> Definition {
    b::function("double")
        .param("self", has_x())
        .returns(ty::int())
        .body(b::add(
            b::field(b::name("self"), "x"),
            b::field(b::name("self"), "x"),
        ))
}

/// `{x, double}`, where `double` is a method.
fn point(x: i64) -> Expr {
    b::record([("x", b::int(x)), ("double", b::method(b::name("double")))])
}

fn main(body: Expr) -> Definition {
    b::function("main").returns(ty::unknown()).body(body)
}

/// What `body` evaluates to, with `double` beside it.
fn evaluates(body: Expr) -> Value {
    run([double(), main(body)], &[]).unwrap()
}

/// The error checking `body` gives.
fn refused(body: Expr) -> Error {
    let module = b::module().define(double()).define(main(body));
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    Engine::new(&workspace, &store)
        .check(&value("main", "main"))
        .unwrap_err()
}

/// A call of the method `name` of `record`.
fn send(record: Expr, name: &str, args: impl IntoIterator<Item = Expr>) -> Expr {
    b::call(b::field(record, name), args)
}

#[test]
fn a_method_receives_the_record_it_is_read_from() {
    assert_eq!(evaluates(send(point(3), "double", [])), Value::Int(6));
}

#[test]
fn a_method_read_and_called_later_still_has_its_record() {
    let read = b::let_(
        "double",
        b::field(point(3), "double"),
        b::call(b::name("double"), []),
    );
    assert_eq!(evaluates(read), Value::Int(6));
}

#[test]
fn a_method_takes_what_follows_its_record() {
    let sum = b::add(b::field(b::name("self"), "x"), b::name("n"));
    let add = b::lambda([("self", Some(has_x())), ("n", Some(ty::int()))], sum);
    let record = b::record([("x", b::int(3)), ("add", b::method(add))]);
    assert_eq!(evaluates(send(record, "add", [b::int(4)])), Value::Int(7));
}

#[test]
fn a_method_reaches_another_through_its_record() {
    let twice = || send(b::name("self"), "double", []);
    let quadruple = b::lambda([("self", Some(doubles()))], b::add(twice(), twice()));
    let record = b::record([
        ("x", b::int(3)),
        ("double", b::method(b::name("double"))),
        ("quadruple", b::method(quadruple)),
    ]);
    assert_eq!(evaluates(send(record, "quadruple", [])), Value::Int(12));
}

#[test]
fn a_function_is_a_method_of_every_record_that_has_what_it_needs() {
    let wider = b::record([
        ("x", b::int(5)),
        ("y", b::int(1)),
        ("double", b::method(b::name("double"))),
    ]);
    let both = b::tuple([send(point(3), "double", []), send(wider, "double", [])]);
    assert_eq!(
        evaluates(both),
        Value::Tuple(vec![Value::Int(6), Value::Int(10)])
    );
}

#[test]
fn a_record_must_have_what_its_method_needs() {
    let record = b::record([("y", b::int(3)), ("double", b::method(b::name("double")))]);
    assert!(matches!(refused(record), Error::Receiver { name, .. } if name == "double"));
}

#[test]
fn a_method_is_a_function_that_takes_its_record() {
    let record = b::record([("double", b::method(b::int(1)))]);
    assert!(matches!(refused(record), Error::Method { .. }));
}

#[test]
fn only_a_field_of_a_record_is_a_method() {
    assert!(matches!(
        refused(b::method(b::name("double"))),
        Error::Method { .. }
    ));
}

#[test]
fn records_with_the_same_methods_and_fields_are_equal() {
    let same = b::tuple([b::equal(point(3), point(3)), b::equal(point(3), point(4))]);
    let expected = Value::Tuple(vec![Value::Bool(true), Value::Bool(false)]);
    assert_eq!(evaluates(same), expected);
}

#[test]
fn a_method_typed_as_a_function_stays_with_its_record_through_a_merge() {
    // `moved` sees `double` as a function, so nothing checked it
    // against the record the merge makes.
    let merged = b::merge(b::name("p"), b::record([("x", b::int(10))]));
    let moved = b::function("moved")
        .param("p", doubles())
        .returns(ty::int())
        .body(send(merged, "double", []));
    let body = b::call(b::name("moved"), [point(3)]);
    assert_eq!(
        run([double(), moved, main(body)], &[]).unwrap(),
        Value::Int(6)
    );
}

#[test]
fn a_method_comes_out_of_unknown_as_the_function_reading_it_gives() {
    let hide = b::function("hide")
        .param("value", ty::unknown())
        .returns(ty::unknown())
        .body(b::name("value"));
    let function = pat::bind_as("f", pat::is(ty::function([], ty::int())));
    let body = b::match_(
        b::call(b::name("hide"), [point(3)]),
        [
            (
                pat::record([("double", function)]),
                b::call(b::name("f"), []),
            ),
            (pat::any(), b::int(-1)),
        ],
    );
    assert_eq!(
        run([double(), hide, main(body)], &[]).unwrap(),
        Value::Int(6)
    );
}

#[test]
fn a_host_sees_a_method_as_a_method() {
    let outcome = evaluates(point(3));
    let Value::Record(fields) = &outcome else {
        panic!("expected a record");
    };
    assert_eq!(fields["x"], Value::Int(3));
    assert!(matches!(fields["double"], Value::Method(_)));
}

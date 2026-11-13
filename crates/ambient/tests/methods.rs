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

/// `self.x + self.x`, a method that does not say what it needs.
fn doubling() -> Expr {
    let x = || b::field(b::name("self"), "x");
    b::lambda_returning([("self", None)], ty::int(), b::add(x(), x()))
}

/// A method that calls `self.double()` and does not say what it needs.
fn redoubling() -> Expr {
    let twice = || send(b::name("self"), "double", []);
    b::lambda([("self", None)], b::add(twice(), twice()))
}

#[test]
fn a_method_that_asks_for_nothing_receives_the_record_it_is_written_in() {
    let record = b::record([("x", b::int(3)), ("double", b::method(doubling()))]);
    assert_eq!(evaluates(send(record, "double", [])), Value::Int(6));
}

#[test]
fn a_method_reaches_one_that_states_what_it_gives() {
    let record = b::record([
        ("quadruple", b::method(redoubling())),
        ("x", b::int(3)),
        ("double", b::method(doubling())),
    ]);
    assert_eq!(evaluates(send(record, "quadruple", [])), Value::Int(12));
}

#[test]
fn a_method_does_not_reach_one_that_leaves_what_it_gives_unstated() {
    let record = b::record([
        ("double", b::method(redoubling())),
        ("quadruple", b::method(redoubling())),
    ]);
    assert!(matches!(refused(record), Error::NoField { name, .. } if name == "double"));
}

#[test]
fn a_method_written_in_a_merge_asks_for_the_merged_record() {
    let added = b::record([("double", b::method(doubling()))]);
    let merged = b::merge(b::record([("x", b::int(4))]), added);
    assert_eq!(evaluates(send(merged, "double", [])), Value::Int(8));
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
fn a_merge_hands_its_methods_the_merged_record() {
    let merged = b::merge(point(3), b::record([("x", b::int(10))]));
    assert_eq!(evaluates(send(merged, "double", [])), Value::Int(20));
}

#[test]
fn a_method_written_in_a_merge_sees_what_is_merged_over() {
    let added = b::record([("double", b::method(b::name("double")))]);
    let merged = b::merge(b::record([("x", b::int(4))]), added);
    assert_eq!(evaluates(send(merged, "double", [])), Value::Int(8));
}

#[test]
fn a_merge_overrides_a_method_for_the_methods_that_reach_it() {
    let twice = || send(b::name("self"), "double", []);
    let quadruple = b::lambda([("self", Some(doubles()))], b::add(twice(), twice()));
    let base = b::record([
        ("x", b::int(3)),
        ("double", b::method(b::name("double"))),
        ("quadruple", b::method(quadruple)),
    ]);
    let hundred = b::lambda([("self", Some(has_x()))], b::int(100));
    let merged = b::merge(base, b::record([("double", b::method(hundred))]));
    assert_eq!(evaluates(send(merged, "quadruple", [])), Value::Int(200));
}

#[test]
fn a_method_taken_from_a_record_that_is_not_written_there_is_still_a_method() {
    // Were `double` bound to `from` by the first merge, the second
    // would not change what it sees.
    let first = b::merge(b::record([("y", b::int(0))]), b::name("from"));
    let second = b::merge(first, b::record([("x", b::int(10))]));
    let body = b::let_("from", point(3), send(second, "double", []));
    assert_eq!(evaluates(body), Value::Int(20));
}

#[test]
fn a_merge_that_takes_what_a_method_needs_is_refused() {
    let merged = b::merge(point(3), b::record([("x", b::string("three"))]));
    assert!(matches!(refused(merged), Error::Receiver { name, .. } if name == "double"));
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

/// `body` in a budget that affords `words` of allocation.
fn affords(words: i64, body: Expr) -> Expr {
    b::budget(b::int(1000), b::int(words), body)
}

#[test]
fn calling_a_method_makes_no_function() {
    // Two words are the sum `double` gives; a function would be four.
    let body = b::let_("p", point(3), affords(2, send(b::name("p"), "double", [])));
    assert_eq!(evaluates(body), Value::Int(6));
}

#[test]
fn reading_a_method_is_charged_for_the_function_it_makes() {
    let read = |words| {
        b::let_(
            "p",
            point(3),
            affords(words, b::field(b::name("p"), "double")),
        )
    };
    assert!(matches!(evaluates(read(4)), Value::Function(_)));
    assert!(matches!(evaluates(read(3)), Value::Tag { .. }));
}

//! Constants. See `docs/spec/constants.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, BuildError, ty};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use support::{program, run, value};

/// What `main` gives when its body is `body`, beside `constants`.
fn evaluates(constants: impl IntoIterator<Item = Definition>, ret: Type, body: Expr) -> Value {
    let main = b::function("main").returns(ret).body(body);
    run(constants.into_iter().chain([main]), &[]).unwrap()
}

/// What a constant of type `of` that holds `written` reads as.
fn reads(of: Type, written: Expr) -> Value {
    let constant = b::constant("HELD", of.clone()).value(written);
    evaluates([constant], of, b::name("HELD"))
}

/// Why `constants` do not hash, each being checked on the way.
fn refused(constants: impl IntoIterator<Item = Definition>) -> Error {
    let workspace = program(constants);
    let store = MemoryStore::new();
    let hashed = Engine::new(&workspace, &store).hash(&value("main", "FIRST"));
    hashed.unwrap_err()
}

fn ints(members: &[i64]) -> Value {
    Value::List(members.iter().copied().map(Value::Int).collect())
}

/// A record type and a record of the same two fields.
fn point() -> (Type, Expr) {
    let of = ty::record([("x", ty::int()), ("label", ty::string())]);
    (
        of,
        b::record([("x", b::int(3)), ("label", b::string("here"))]),
    )
}

#[test]
fn naming_a_constant_gives_its_value() {
    assert_eq!(reads(ty::int(), b::int(7)), Value::Int(7));
}

#[test]
fn a_constant_holds_numbers_of_any_size_and_text() {
    assert_eq!(reads(ty::int(), b::int(i64::MAX)), Value::Int(i64::MAX));
    assert_eq!(reads(ty::float(), b::float(1.5)), Value::Float(1.5));
    let text = "longer than one word";
    assert_eq!(
        reads(ty::string(), b::string(text)),
        Value::String(text.to_owned())
    );
}

#[test]
fn a_constant_holds_tuples_and_lists() {
    let of = ty::tuple([ty::list(ty::int()), ty::list(ty::int())]);
    let written = b::tuple([b::list([b::int(1), b::int(2)]), b::list([])]);
    assert_eq!(
        reads(of, written),
        Value::Tuple(vec![ints(&[1, 2]), ints(&[])])
    );
}

#[test]
fn pushing_onto_a_constant_list_leaves_the_constant_as_it_was() {
    let list = b::constant("LIST", ty::list(ty::int())).value(b::list([b::int(1)]));
    let pushed = b::push(b::name("LIST"), b::int(2));
    let of = ty::tuple([ty::list(ty::int()), ty::list(ty::int())]);
    let both = evaluates([list], of, b::tuple([pushed, b::name("LIST")]));
    assert_eq!(both, Value::Tuple(vec![ints(&[1, 2]), ints(&[1])]));
}

#[test]
fn a_constant_record_is_the_record_a_function_would_make() {
    let (of, written) = point();
    let constant = b::constant("POINT", of).value(written.clone());
    let same = b::equal(b::name("POINT"), written);
    assert_eq!(evaluates([constant], ty::bool(), same), Value::Bool(true));
}

#[test]
fn a_field_of_a_constant_record_is_read() {
    let (of, written) = point();
    let constant = b::constant("POINT", of).value(written);
    let label = b::field(b::name("POINT"), "label");
    let read = evaluates([constant], ty::string(), label);
    assert_eq!(read, Value::String("here".to_owned()));
}

#[test]
fn a_constant_holds_another() {
    let inner = b::constant("INNER", ty::list(ty::int())).value(b::list([b::int(4)]));
    let of = ty::tuple([ty::list(ty::int()), ty::int()]);
    let outer = b::constant("OUTER", of.clone()).value(b::tuple([b::name("INNER"), b::int(5)]));
    let read = evaluates([inner, outer], of, b::name("OUTER"));
    assert_eq!(read, Value::Tuple(vec![ints(&[4]), Value::Int(5)]));
}

#[test]
fn a_constant_holds_functions_to_call() {
    let twice = b::function("twice")
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::add(b::name("n"), b::name("n")));
    let step = ty::function([ty::int()], ty::int());
    let next = b::lambda([("n", Some(ty::int()))], b::add(b::name("n"), b::int(1)));
    let steps = b::constant(
        "STEPS",
        ty::record([("twice", step.clone()), ("next", step)]),
    )
    .value(b::record([("twice", b::name("twice")), ("next", next)]));
    let step = |name, of| b::call(b::field(b::name("STEPS"), name), [of]);
    let both = step("next", step("twice", b::int(5)));
    assert_eq!(evaluates([twice, steps], ty::int(), both), Value::Int(11));
}

#[test]
fn a_function_a_constant_holds_may_name_the_constant() {
    let of = ty::record([("size", ty::int()), ("get", ty::function([], ty::int()))]);
    let get = b::lambda_returning([], ty::int(), b::field(b::name("SELF"), "size"));
    let held = b::constant("SELF", of).value(b::record([("size", b::int(9)), ("get", get)]));
    let got = b::call(b::field(b::name("SELF"), "get"), []);
    assert_eq!(evaluates([held], ty::int(), got), Value::Int(9));
}

#[test]
fn a_constant_that_holds_itself_is_refused() {
    let first = b::constant("FIRST", ty::int()).value(b::name("SECOND"));
    let second = b::constant("SECOND", ty::int()).value(b::name("FIRST"));
    assert!(matches!(refused([first, second]), Error::ConstantCycle(_)));
}

#[test]
fn a_constant_that_computes_is_refused() {
    let sum = b::constant("FIRST", ty::int()).value(b::add(b::int(1), b::int(2)));
    assert!(matches!(refused([sum]), Error::NotConstant(_)));
}

#[test]
fn a_constant_is_held_to_the_type_it_states() {
    let wrong = b::constant("FIRST", ty::int()).value(b::string("one"));
    assert!(matches!(refused([wrong]), Error::Mismatch { .. }));
}

#[test]
fn a_constant_is_named_in_capitals() {
    let lower = b::constant("limit", ty::int()).value(b::int(1));
    let built = b::module().define(lower).build();
    assert_eq!(
        built.unwrap_err(),
        BuildError::ConstantName("limit".to_owned())
    );
}

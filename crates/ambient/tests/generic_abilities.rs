//! Abilities whose signatures have type parameters, run on every
//! backend. See `spec/abilities.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::backend::native::Natives;
use ambient::builder::{self as b, pat, ty};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use support::{run, run_with, value, workspace};

fn t() -> Type {
    ty::named("T")
}

/// `ability pick<T>(first: T, second: T): T`
fn pick() -> Definition {
    b::ability("pick")
        .type_param("T")
        .param("first", t())
        .param("second", t())
        .returns(t())
        .into()
}

/// `ability alike<T>(like: T, v: unknown): bool`
fn alike() -> Definition {
    b::ability("alike")
        .type_param("T")
        .param("like", t())
        .param("v", ty::unknown())
        .returns(ty::bool())
        .into()
}

/// `(pick(1, 2), pick("a", "b"))`
fn picks() -> Expr {
    let ints = b::perform("pick", [b::int(1), b::int(2)]);
    let strings = b::perform("pick", [b::string("a"), b::string("b")]);
    b::tuple([ints, strings])
}

/// `<T>(first, second) => second`, its parameters typed by the ability.
fn second() -> Expr {
    let params = [("first", None), ("second", None)];
    b::generic([("T", None)], b::lambda(params, b::name("second")))
}

fn seconds() -> Value {
    Value::Tuple(vec![Value::Int(2), Value::String("b".into())])
}

fn main(body: Expr) -> Definition {
    b::function("main").returns(ty::unknown()).body(body)
}

fn evaluates(body: Expr) -> Value {
    run([pick(), alike(), main(body)], &[]).unwrap()
}

/// The error checking `main` gives, beside `definitions`.
fn refused(definitions: impl IntoIterator<Item = Definition>) -> Error {
    let module = definitions
        .into_iter()
        .fold(b::module(), b::ModuleBuilder::define);
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    Engine::new(&workspace, &store)
        .check(&value("main", "main"))
        .unwrap_err()
}

#[test]
fn a_generic_ability_is_performed_at_more_than_one_type() {
    let body = b::provide([("pick", second())], picks());
    assert_eq!(evaluates(body), seconds());
}

#[test]
fn a_handler_for_a_generic_ability_is_a_generic_function() {
    let ints = [("first", Some(ty::int())), ("second", Some(ty::int()))];
    let handler = b::lambda(ints, b::name("second"));
    let body = b::provide(
        [("pick", handler)],
        b::perform("pick", [b::int(1), b::int(2)]),
    );
    assert!(matches!(
        refused([pick(), main(body)]),
        Error::Mismatch { .. }
    ));
}

#[test]
fn a_handler_that_needs_its_type_arguments_is_handed_them() {
    let arms = [
        (pat::is(t()), b::boolean(true)),
        (pat::any(), b::boolean(false)),
    ];
    let params = [("like", None), ("v", None)];
    let handler = b::generic(
        [("T", None)],
        b::lambda(params, b::match_(b::name("v"), arms)),
    );
    // A sum is an `int`, where a literal would be only itself.
    let int = || b::add(b::int(0), b::int(1));
    let tests = b::tuple([
        b::perform("alike", [int(), b::int(2)]),
        b::perform("alike", [int(), b::string("a")]),
    ]);
    let expected = [true, false].map(Value::Bool);
    assert_eq!(
        evaluates(b::provide([("alike", handler)], tests)),
        Value::Tuple(expected.to_vec())
    );
}

#[test]
fn a_recording_forwards_a_generic_ability_at_its_type_arguments() {
    let observer = b::lambda([("event", None)], b::null());
    let body = b::provide([("pick", second())], b::recording(observer, picks()));
    assert_eq!(evaluates(body), seconds());
}

#[test]
fn a_target_binds_a_generic_function_to_a_generic_ability() {
    let later = b::function("later")
        .type_param("T")
        .param("first", t())
        .param("second", t())
        .returns(t())
        .body(b::name("second"));
    let entry = b::function("main")
        .performs(["pick"])
        .returns(ty::unknown())
        .body(picks());
    let target = b::target("target").bind("pick", "later");
    let definitions = [pick(), later, target.into(), entry];
    assert_eq!(
        run_with(Some("target"), definitions, Natives::new),
        Ok(seconds())
    );
}

#[test]
fn an_ability_that_does_not_return_takes_no_type_parameters() {
    let fail = b::ability("fail")
        .type_param("T")
        .param("reason", t())
        .returns(ty::never());
    let body = b::perform("fail", [b::int(1)]);
    let main = b::function("main")
        .performs(["fail"])
        .returns(ty::unknown());
    let error = refused([fail.into(), main.body(body)]);
    assert!(matches!(error, Error::GenericAbort(_)), "{error}");
}

#[test]
fn a_generic_function_that_takes_an_ability_is_bound_in_place_as_a_handler() {
    // `fn noted<T>(first: T, second: T): [note] T`
    let note = b::ability("note").returns(ty::null());
    let noted = b::function("noted")
        .type_param("T")
        .param("first", t())
        .param("second", t())
        .performs(["note"])
        .returns(t())
        .body(b::let_("done", b::perform("note", []), b::name("second")));
    let handled = b::provide([("pick", b::name("noted"))], picks());
    let body = b::provide([("note", b::lambda([], b::null()))], handled);
    let definitions = [pick(), note.into(), noted, main(body)];
    assert_eq!(run(definitions, &[]).unwrap(), seconds());
}

//! Default bindings: a native handler that answers its ability under
//! the targets it names. See "Default bindings" in
//! `docs/spec/targets.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::backend::native::Natives;
use ambient::builder::{self as b, ModuleBuilder, ty};
use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::Definition;
use ambient::value::Value;
use ambient_runtime::Fault;
use support::{run_with, value, workspace};

/// `ability ask(): int`.
fn ask() -> Definition {
    b::ability("ask").returns(ty::int()).into()
}

/// `fn main(): int with ask`, which asks.
fn main() -> Definition {
    b::function("main")
        .performs(["ask"])
        .returns(ty::int())
        .body(b::perform("ask", []))
}

/// A native handler for `ask`, known by the ability's own name, that
/// is its default under `plain`.
fn answering() -> Definition {
    b::native_handler("ask", "ask").under("plain").into()
}

/// Native code for [`answering`], which gives 7.
fn seven() -> Natives {
    Natives::new().with("app", "main.ask", |_| Ok(Value::Int(7)))
}

/// Runs `main` under `target`, beside `ask`, the target `plain`, and
/// `definitions`.
fn runs(definitions: impl IntoIterator<Item = Definition>) -> Result<Value, Fault> {
    let given = [ask(), b::target("plain").into(), main()];
    run_with(Some("target"), given.into_iter().chain(definitions), seven)
}

/// Checks the program of `main` under `target`, as [`runs`] runs it.
fn checks(definitions: impl IntoIterator<Item = Definition>) -> Result<(), Error> {
    let given = [ask(), b::target("plain").into(), main()];
    let module = given
        .into_iter()
        .chain(definitions)
        .fold(b::module(), ModuleBuilder::define);
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let root = engine.program(&value("main", "main"), &value("main", "target"))?;
    engine.check(&root).map(|_| ())
}

#[test]
fn a_default_answers_under_a_target_that_extends_the_one_it_names() {
    let target = b::target("target").extends("plain");

    assert_eq!(runs([answering(), target.into()]), Ok(Value::Int(7)));
}

#[test]
fn what_a_target_binds_is_taken_before_a_default() {
    let two = b::function("two").returns(ty::int()).body(b::int(2));
    let target = b::target("target").extends("plain").bind("ask", "two");

    assert_eq!(runs([answering(), two, target.into()]), Ok(Value::Int(2)));
}

#[test]
fn a_target_takes_no_default_that_names_nothing_it_extends() {
    let apart = b::target("target");

    let outcome = checks([answering(), apart.into()]);
    assert!(
        matches!(outcome, Err(Error::Unoffered { .. })),
        "{outcome:?}"
    );
}

#[test]
fn a_default_is_reached_through_the_row_of_a_handler() {
    // `fn doubled(): int with ask`, bound to an ability of its own,
    // which is all the entry asks for.
    let twice = b::ability("twice").returns(ty::int());
    let doubled = b::function("doubled")
        .performs(["ask"])
        .returns(ty::int())
        .body(b::multiply(b::perform("ask", []), b::int(2)));
    let main = b::function("main")
        .performs(["twice"])
        .returns(ty::int())
        .body(b::perform("twice", []));
    let target = b::target("target")
        .extends("plain")
        .bind("twice", "doubled");
    let given = [ask(), b::target("plain").into(), answering()];
    let definitions = given
        .into_iter()
        .chain([twice.into(), doubled, main, target.into()]);

    let outcome = run_with(Some("target"), definitions, seven);
    assert_eq!(outcome, Ok(Value::Int(14)));
}

#[test]
fn two_defaults_under_one_target_are_a_conflict_a_binding_settles() {
    let other = b::native_handler("other", "ask").under("plain");
    let unsettled = b::target("target").extends("plain");
    let settled = b::target("target").extends("plain").bind("ask", "ask");

    let outcome = checks([answering(), other.clone().into(), unsettled.into()]);
    assert!(
        matches!(outcome, Err(Error::TargetConflict { .. })),
        "{outcome:?}"
    );
    assert_eq!(checks([answering(), other.into(), settled.into()]), Ok(()));
}

#[test]
fn a_default_names_a_target() {
    let handler = b::native_handler("ask", "ask").under("main");
    let workspace = workspace([(
        "app",
        "main",
        b::module().define(ask()).define(main()).define(handler),
    )]);
    let store = MemoryStore::new();

    let outcome = Engine::new(&workspace, &store).check(&value("main", "ask"));
    assert!(matches!(outcome, Err(Error::NotTarget(_))), "{outcome:?}");
}

#[test]
fn only_the_package_of_an_ability_gives_it_a_default() {
    let library = b::module().define(b::ability("ask").public().returns(ty::int()));
    let handler = b::native_handler("answer", "ask").under("plain");
    let main = b::module()
        .import("lib.ask")
        .define(b::target("plain"))
        .define(handler);
    let mut workspace = workspace([("lib", "", library), ("app", "main", main)]);
    workspace.depend("app", "lib", "lib").unwrap();
    let store = MemoryStore::new();

    let outcome = Engine::new(&workspace, &store).check(&value("main", "answer"));
    assert!(
        matches!(outcome, Err(Error::ForeignDefault { .. })),
        "{outcome:?}"
    );
}

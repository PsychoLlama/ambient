//! What an edit invalidates. See `spec/definitions.md` and `spec/build.md`.
//!
//! Each test builds a program, edits it, and builds again over the same
//! store. The second engine's log says which stages did their work.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, ModuleBuilder, ty};
use ambient::engine::{Engine, Stage};
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::Definition;
use ambient::symbol::Symbol;
use ambient::workspace::Workspace;
use support::{Recording, value, workspace};

fn constant(name: &str, result: i64) -> Definition {
    b::function(name).returns(ty::int()).body(b::int(result))
}

fn calls(name: &str, callee: &str) -> Definition {
    b::function(name)
        .returns(ty::int())
        .body(b::call(b::name(callee), []))
}

/// `top` calls `mid` calls `leaf`.
fn chain(leaf: Definition) -> ModuleBuilder {
    b::module()
        .define(leaf)
        .define(calls("mid", "leaf"))
        .define(calls("top", "mid"))
}

fn names(symbols: &[&str]) -> Vec<String> {
    symbols
        .iter()
        .map(|name| format!("app:main.{name}"))
        .collect()
}

/// Hashes and compiles every symbol, as a whole-program build would.
fn build(engine: &Engine, symbols: &[Symbol]) -> Vec<ambient::hash::Hash> {
    let backend = Recording::default();
    symbols
        .iter()
        .map(|symbol| {
            engine.compile(symbol, &backend).unwrap();
            engine.hash(symbol).unwrap()
        })
        .collect()
}

fn chain_symbols() -> [Symbol; 3] {
    [
        value("main", "leaf"),
        value("main", "mid"),
        value("main", "top"),
    ]
}

fn main(module: ModuleBuilder) -> Workspace {
    workspace([("app", "main", module)])
}

#[test]
fn only_what_the_root_reaches_is_touched() {
    let module = chain(constant("leaf", 1)).define(calls("broken", "missing"));
    let workspace = main(module);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);

    engine.check(&value("main", "top")).unwrap();

    // Checking `top` needs `mid`'s header, and nothing of `leaf`.
    assert_eq!(engine.ran(Stage::Resolve), names(&["mid", "top"]));
    assert_eq!(engine.ran(Stage::Check), names(&["top"]));
}

#[test]
fn a_body_edit_rechecks_and_recompiles_one_definition() {
    let store = MemoryStore::new();
    let before = main(chain(constant("leaf", 1)));
    let old = build(&Engine::new(&before, &store), &chain_symbols());

    let after = main(chain(constant("leaf", 2)));
    let engine = Engine::new(&after, &store);
    let new = build(&engine, &chain_symbols());

    assert_eq!(engine.ran(Stage::Check), names(&["leaf"]));
    assert_eq!(engine.ran(Stage::Compile), names(&["leaf"]));
    assert_eq!(engine.reused(Stage::Check), names(&["mid", "top"]));
    assert_eq!(engine.reused(Stage::Compile), names(&["mid", "top"]));

    // Dependents get new hashes all the same.
    assert_ne!(old[1], new[1]);
    assert_ne!(old[2], new[2]);
}

#[test]
fn a_header_edit_rechecks_direct_dependents_and_stops() {
    let store = MemoryStore::new();
    let before = main(chain(constant("leaf", 1)));
    build(&Engine::new(&before, &store), &chain_symbols());

    let leaf = b::function("leaf").returns(ty::float()).body(b::int(1));
    let after = main(chain(leaf));
    let engine = Engine::new(&after, &store);
    build(&engine, &chain_symbols());

    assert_eq!(engine.ran(Stage::Check), names(&["leaf", "mid"]));
    assert_eq!(engine.ran(Stage::Compile), names(&["leaf", "mid"]));
    assert_eq!(engine.reused(Stage::Check), names(&["top"]));
}

#[test]
fn a_type_edit_propagates_through_interfaces_that_name_it() {
    let program = |field: &str| {
        b::module()
            .define(b::type_("Point", ty::record([(field, ty::int())])))
            .define(
                b::function("origin")
                    .returns(ty::named("Point"))
                    .body(b::record([(field, b::int(0))])),
            )
            .define(b::function("uses_origin").returns(ty::int()).body(b::let_(
                "p",
                b::call(b::name("origin"), []),
                b::int(0),
            )))
            .define(calls("far", "uses_origin"))
    };
    let symbols = [
        value("main", "origin"),
        value("main", "uses_origin"),
        value("main", "far"),
    ];
    let store = MemoryStore::new();
    build(&Engine::new(&main(program("x")), &store), &symbols);

    let after = main(program("y"));
    let engine = Engine::new(&after, &store);
    build(&engine, &symbols);

    // `origin` names the type; `uses_origin` depends on `origin`'s
    // interface; `far` depends on an interface that does not name it.
    assert_eq!(
        engine.ran(Stage::Check),
        names(&["Point", "origin", "uses_origin"])
    );
    assert_eq!(engine.reused(Stage::Check), names(&["far"]));
}

#[test]
fn renaming_a_local_recompiles_nothing() {
    let identity = |param: &str| {
        b::module().define(
            b::function("id")
                .param(param, ty::int())
                .returns(ty::int())
                .body(b::name(param)),
        )
    };
    let id = [value("main", "id")];
    let store = MemoryStore::new();
    let old = build(&Engine::new(&main(identity("x")), &store), &id);

    let after = main(identity("y"));
    let engine = Engine::new(&after, &store);
    let new = build(&engine, &id);

    assert_eq!(engine.ran(Stage::Compile), names(&[]));
    assert_eq!(old, new);
}

#[test]
fn adding_a_definition_rechecks_nothing() {
    let store = MemoryStore::new();
    build(
        &Engine::new(&main(chain(constant("leaf", 1))), &store),
        &chain_symbols(),
    );

    let after = main(chain(constant("leaf", 1)).define(constant("extra", 9)));
    let engine = Engine::new(&after, &store);
    build(&engine, &chain_symbols());

    assert_eq!(engine.ran(Stage::Check), names(&[]));
    assert_eq!(engine.ran(Stage::Compile), names(&[]));
}

#[test]
fn a_stored_result_is_valid_in_any_workspace() {
    let store = MemoryStore::new();
    let first = main(b::module().define(constant("leaf", 1)));
    build(&Engine::new(&first, &store), &[value("main", "leaf")]);

    // The same definition in another module of another program.
    let second = workspace([(
        "app",
        "elsewhere.deep",
        b::module().define(constant("leaf", 1)),
    )]);
    let engine = Engine::new(&second, &store);
    build(&engine, &[value("elsewhere.deep", "leaf")]);

    assert_eq!(engine.ran(Stage::Check), names(&[]));
    assert_eq!(engine.ran(Stage::Compile), names(&[]));
}

#[test]
fn an_engine_answers_each_question_once() {
    let workspace = main(chain(constant("leaf", 1)));
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);

    build(&engine, &chain_symbols());
    let events = engine.events().len();
    build(&engine, &chain_symbols());

    assert_eq!(engine.events().len(), events);
}

#[test]
fn an_error_is_a_stored_result() {
    let workspace = main(b::module().define(calls("broken", "missing")));
    let store = MemoryStore::new();
    let broken = value("main", "broken");

    let first = Engine::new(&workspace, &store).check(&broken).unwrap_err();
    let engine = Engine::new(&workspace, &store);
    let second = engine.check(&broken).unwrap_err();

    assert!(matches!(first, Error::UnknownName { .. }));
    assert_eq!(first, second);
    assert_eq!(engine.ran(Stage::Resolve), names(&[]));
}

#[test]
fn a_backend_is_not_asked_twice() {
    let workspace = main(chain(constant("leaf", 1)));
    let store = MemoryStore::new();
    let backend = Recording::default();

    for _ in 0..2 {
        let engine = Engine::new(&workspace, &store);
        for symbol in chain_symbols() {
            engine.compile(&symbol, &backend).unwrap();
        }
    }

    // `mid` and `top` have the same tree and call the same interface, so
    // they are one artifact.
    assert_eq!(*backend.compiled.borrow(), 2);
}

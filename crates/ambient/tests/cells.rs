//! How long a cell lives in a native host. See "Memory" in
//! `spec/backends.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::backend::native::{Host, Native, Natives, Program};
use ambient::builder::{self as b, ty};
use ambient::engine::Engine;
use ambient::store::MemoryStore;
use ambient::surface::Definition;
use ambient::value::{Value, decode};
use ambient_runtime::{NULL, amb_live, amb_release};
use support::value;

/// Loads `main`, alone in its module.
fn load(main: Definition) -> Program {
    let workspace = support::workspace([("app", "main", b::module().define(main))]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let parts = engine.link(&value("main", "main"), &Native::new().unwrap());
    Program::load(&parts.unwrap(), &Natives::new()).unwrap()
}

#[test]
fn a_cell_outlives_the_call_that_made_it() {
    // Given a cell, `main` reads it; given anything else, it makes one.
    let made = b::cell(ty::int(), b::int(7));
    let given = b::cast(b::name("c"), ty::cell(ty::int()), b::null());
    let arms = [
        (b::pat::is(ty::null()), made),
        (b::pat::bind("it"), b::read(b::name("it"))),
    ];
    let main = b::function("main")
        .param("c", ty::unknown())
        .returns(ty::unknown())
        .body(b::let_("held", given, b::match_(b::name("held"), arms)));
    let program = load(main);

    let cell = program.call(&[NULL]).unwrap();
    let read = program.call(&[cell]).unwrap();

    assert_eq!(decode(read, &mut Host), Value::Int(7));
    for word in [cell, read] {
        // SAFETY: the word is owned here and not used again.
        unsafe { amb_release(word) };
    }
}

#[test]
fn a_cell_that_reaches_itself_is_reclaimed_with_its_program() {
    // A cell that holds a list of itself: counting never reclaims it.
    let tied = b::replace(b::name("c"), b::null(), b::list([b::name("c")]));
    let main = b::function("main").returns(ty::bool()).body(b::let_(
        "c",
        b::cell(ty::unknown(), b::null()),
        tied,
    ));
    let program = load(main);
    let before = amb_live();

    program.call(&[]).unwrap();
    let during = amb_live();
    drop(program);

    assert!(during > before, "the cell did not reach itself");
    assert_eq!(amb_live(), before);
}

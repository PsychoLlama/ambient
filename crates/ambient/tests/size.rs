//! How compiled code grows with its source. See "Suspension" in
//! `spec/backends.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::backend::native::Native;
use ambient::backend::wasm::Wasm;
use ambient::builder::{self as b, ty};
use ambient::engine::{Backend, Engine};
use ambient::store::MemoryStore;
use support::value;

/// How many bytes `main` compiles to when it makes a list of `calls`
/// calls: each a point it may suspend at, with every earlier result held
/// in a local.
fn size(backend: &dyn Backend, calls: i64) -> usize {
    let same = b::function("same")
        .param("v", ty::int())
        .returns(ty::int())
        .body(b::name("v"));
    let items = (0..calls).map(|item| b::call(b::name("same"), [b::int(item)]));
    let main = b::function("main")
        .returns(ty::list(ty::int()))
        .body(b::list(items));
    let workspace = support::workspace([("app", "main", b::module().define(same).define(main))]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    engine
        .compile(&value("main", "main"), backend)
        .unwrap()
        .0
        .len()
}

#[test]
fn code_grows_in_proportion_to_its_source() {
    let backends: [&dyn Backend; 2] = [&Native::new().unwrap(), &Wasm];

    for backend in backends {
        let (small, large) = (size(backend, 50), size(backend, 100));
        // Not with the product of points and locals, which would be
        // four times.
        assert!(
            large < small * 5 / 2,
            "{}: {small} to {large}",
            backend.name()
        );
    }
}

#[test]
fn a_function_far_longer_than_the_stack_is_deep_compiles() {
    // Each member is bound before the list is made, so lowered code
    // nests once for each: what translates it must not run out of stack.
    let members = (0..4000).map(|member| b::list([b::int(member)]));
    let main = b::function("main")
        .returns(ty::unknown())
        .body(b::list(members));
    let workspace = support::workspace([("app", "main", b::module().define(main))]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let backends: [&dyn Backend; 2] = [&Native::new().unwrap(), &Wasm];

    for backend in backends {
        assert!(engine.link(&value("main", "main"), backend).is_ok());
    }
}

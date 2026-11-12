//! How deeply a tree may nest. See "Limits" in `spec/core.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, BuildError, ty};
use ambient::core::{DEPTH, Expr, Tree};
use ambient::engine::Engine;
use ambient::pack::{Malformed, Pack};
use ambient::store::MemoryStore;
use ambient::surface::Definition;
use ambient::value::Value;
use support::{value, workspace};

/// `fn main(x: int): int`, which adds one to `x` in each of `bindings`
/// bindings, every one inside the last. The innermost addition is two
/// levels further in.
fn counting(bindings: usize) -> Definition {
    let body = (0..bindings).fold(b::name("x"), |body, _| {
        b::let_("x", b::add(b::name("x"), b::int(1)), body)
    });
    b::function("main")
        .param("x", ty::int())
        .returns(ty::int())
        .body(body)
}

/// A pack of `main`, as its sender made it.
fn packed(main: Definition) -> Pack {
    let workspace = workspace([("app", "main", b::module().define(main))]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    engine.pack(&value("main", "main"), &|_| false).unwrap()
}

#[test]
fn a_function_as_deep_as_a_tree_may_be_runs() {
    let bindings = DEPTH - 2;
    let count = i64::try_from(bindings).unwrap();
    assert_eq!(
        support::run([counting(bindings)], &[0]),
        Ok(Value::Int(count))
    );
}

#[test]
fn a_builder_refuses_a_deeper_one() {
    let module = b::module().define(counting(DEPTH - 1));
    assert_eq!(module.build().err(), Some(BuildError::Nesting));
}

#[test]
fn a_pack_as_deep_as_a_tree_may_be_is_received() {
    let pack = Pack::decode(&packed(counting(DEPTH - 2)).encode()).unwrap();
    let workspace = workspace([("app", "main", b::module())]);
    let store = MemoryStore::new();
    assert!(Engine::new(&workspace, &store).receive(&pack, &[]).is_ok());
}

#[test]
fn a_pack_that_nests_deeper_is_not_a_pack() {
    let mut pack = packed(counting(DEPTH - 2));
    let Tree::Function(main) = &mut pack.definitions[0].tree else {
        unreachable!("the entry is a function");
    };
    main.body = Expr::Boundary(Box::new(std::mem::replace(&mut main.body, Expr::Null)));
    assert_eq!(Pack::decode(&pack.encode()), Err(Malformed));
}

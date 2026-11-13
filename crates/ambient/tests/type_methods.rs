//! The methods a type definition has by the module it is defined in. See
//! `spec/methods.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, ModuleBuilder, pat, ty};
use ambient::engine::{Engine, Stage};
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use ambient::workspace::Workspace;
use support::{run_in, value, workspace};

fn items_of(member: Type) -> Type {
    ty::apply("Items", [member])
}

/// A function of the module `items` that takes a list first.
fn of_items(name: &str) -> b::FunctionBuilder {
    b::function(name)
        .type_param("T")
        .param("self", items_of(ty::named("T")))
}

/// `fn size<T>(self: Items<T>): R`, how many members a list has.
fn size(returns: Type) -> Definition {
    let rest = b::call(b::name("size"), [b::name("rest")]);
    let arms = [
        (pat::is(ty::null()), b::int(0)),
        (
            pat::record([("tail", pat::bind("rest"))]),
            b::add(b::int(1), rest),
        ),
    ];
    of_items("size")
        .public()
        .returns(returns)
        .body(b::match_(b::name("self"), arms))
}

/// The module `items`: `type Items<T> = null | {head: T, tail: Items<T>}`,
/// `cons` to make one, a private `hidden`, and `functions`.
fn items(functions: impl IntoIterator<Item = Definition>) -> ModuleBuilder {
    let member = || ty::named("T");
    let node = ty::record([("head", member()), ("tail", items_of(member()))]);
    let cons = of_items("cons")
        .public()
        .param("head", member())
        .returns(items_of(member()))
        .body(b::record([
            ("head", b::name("head")),
            ("tail", b::name("self")),
        ]));
    let module = b::module()
        .define(b::generic_type("Items", ["T"], ty::union([ty::null(), node])).public())
        .define(cons)
        .define(of_items("hidden").returns(ty::int()).body(b::int(0)));
    functions.into_iter().fold(module, ModuleBuilder::define)
}

/// A list of one member, made in another module.
fn one() -> Expr {
    b::call(b::name("items.cons"), [b::null(), b::int(1)])
}

/// A program whose `main` gives `body`, beside this `items` module.
fn program(items: ModuleBuilder, body: Expr) -> Workspace {
    let main = b::function("main").returns(ty::unknown()).body(body);
    let module = b::module().import("pkg.items").define(main);
    workspace([("app", "items", items), ("app", "main", module)])
}

fn sized() -> Expr {
    b::send(one(), "size", [])
}

#[test]
fn a_type_has_the_public_functions_of_its_module_as_methods() {
    let program = program(items([size(ty::int())]), sized());
    assert_eq!(run_in(&program).unwrap(), Value::Int(1));
}

#[test]
fn a_method_is_the_receiver_of_another() {
    let two = b::send(one(), "cons", [b::int(2)]);
    let program = program(items([size(ty::int())]), b::send(two, "size", []));
    assert_eq!(run_in(&program).unwrap(), Value::Int(2));
}

#[test]
fn a_private_function_is_no_method() {
    let program = program(items([]), b::send(one(), "hidden", []));
    let store = MemoryStore::new();
    let refused = Engine::new(&program, &store).check(&value("main", "main"));
    assert!(matches!(refused, Err(Error::NoMethod { name, .. }) if name == "hidden"));
}

/// Which definitions the second of two builds of `main` checks again.
fn rechecked(before: ModuleBuilder, after: ModuleBuilder) -> Vec<String> {
    let store = MemoryStore::new();
    let main = value("main", "main");
    let built = program(before, sized());
    Engine::new(&built, &store).check(&main).unwrap();
    let edited = program(after, sized());
    let engine = Engine::new(&edited, &store);
    engine.check(&main).unwrap();
    engine.ran(Stage::Check)
}

#[test]
fn a_method_added_to_a_module_rechecks_no_call_of_another() {
    let total = of_items("total")
        .public()
        .returns(ty::int())
        .body(b::int(0));
    let after = items([size(ty::int()), total]);
    assert!(rechecked(items([size(ty::int())]), after).is_empty());
}

#[test]
fn a_method_whose_header_changes_rechecks_what_calls_it() {
    let after = items([size(ty::unknown())]);
    let rechecked = rechecked(items([size(ty::int())]), after);
    assert_eq!(rechecked, ["app:main.main"]);
}

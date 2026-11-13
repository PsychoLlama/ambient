//! How large a compiled WebAssembly program is. See "Floor" in
//! `spec/linking.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::backend::wasm::{HOST, Wasm, module_for};
use ambient::builder::{self as b, ty};
use ambient::engine::Engine;
use ambient::library;
use ambient::store::MemoryStore;
use ambient::surface::Expr;
use support::{value, workspace};

/// What a host that starts a call and reads nothing calls.
const BARE: &[&str] = &[
    "amb_context_new",
    "amb_context_free",
    "amb_start",
    "amb_release",
];

/// The bytes of the module `main`, with this body, compiles to, for a
/// host that calls `host`.
fn size(body: Expr, host: &[&'static str]) -> usize {
    let main = b::function("main").returns(ty::unknown()).body(body);
    let module = b::module().import("ambient.iterator").define(main);
    let mut workspace = workspace([("app", "main", module)]);
    workspace.depend("app", "ambient", "ambient").unwrap();
    library::install(&mut workspace).unwrap();
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let parts = engine.link(&value("main", "main"), &Wasm).unwrap();
    module_for(&parts, host).unwrap().len()
}

fn fold() -> Expr {
    let range = b::call(b::name("iterator.range"), [b::int(0), b::int(10)]);
    let params = [("sum", Some(ty::int())), ("next", Some(ty::int()))];
    let add = b::lambda(params, b::add(b::name("sum"), b::name("next")));
    b::call(b::name("iterator.fold"), [range, b::int(0), add])
}

/// The most a program that does nothing may be, for a host that
/// starts a call and reads nothing. Lower it when the floor falls;
/// raising it is a decision, not a fix.
const FLOOR: usize = 24_000;

#[test]
fn a_program_that_does_nothing_is_within_the_floor() {
    let size = size(b::null(), BARE);
    assert!(size <= FLOOR, "{size} bytes");
}

#[test]
fn a_program_pays_for_what_it_reaches_and_no_more() {
    let (nothing, text) = (size(b::null(), BARE), size(b::string("hello"), BARE));
    // A string that nothing is done with brings what makes it.
    assert!(text - nothing < 128, "{} bytes", text - nothing);
    assert!(size(fold(), BARE) > text);
}

#[test]
fn a_host_that_reads_no_values_pays_for_no_readers() {
    assert!(size(b::null(), BARE) < size(b::null(), HOST));
}

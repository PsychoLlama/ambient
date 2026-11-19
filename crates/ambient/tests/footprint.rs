//! How large a compiled WebAssembly program is. See "Floor" in
//! `docs/spec/linking.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::backend::wasm::{HOST, Wasm, module_for, pieces};
use ambient::builder::{self as b, ty};
use ambient::engine::Engine;
use ambient::engine::Parts;
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

/// The parts of the program `main`, with this body, is, beside the
/// standard library.
fn program(body: Expr) -> Parts {
    let main = b::function("main").returns(ty::unknown()).body(body);
    let module = b::module().import("ambient.iterator").define(main);
    let mut workspace = workspace([("app", "main", module)]);
    workspace.depend("app", "ambient", "ambient").unwrap();
    library::install(&mut workspace).unwrap();
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    engine.link(&value("main", "main"), &Wasm).unwrap()
}

/// The bytes of the module that program compiles to, for a host that
/// calls `host`.
fn size(body: Expr, host: &[&'static str]) -> usize {
    module_for(&program(body), host).unwrap().len()
}

/// Whether that program, for the host in this crate, carries the
/// runtime's function `name`.
fn carries(body: Expr, name: &str) -> bool {
    let pieces = pieces(&program(body), HOST).unwrap();
    pieces.iter().any(|piece| piece.name == name)
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
///
const FLOOR: usize = 7_600;

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

#[test]
fn only_a_program_that_can_suspend_carries_what_gives_tasks_their_turns() {
    assert!(!carries(b::null(), "amb_schedule"));
    assert!(carries(b::all([b::int(1), b::int(2)]), "amb_schedule"));
}

#[test]
fn only_a_program_that_can_suspend_is_compiled_to() {
    // What keeps a function's locals while its task is suspended.
    assert!(!carries(fold(), "amb_frame_push"));
    assert!(carries(b::all([b::int(1), b::int(2)]), "amb_frame_push"));
}

/// A list of `count` functions, each a definition of its own.
fn functions(count: i64) -> Expr {
    let param = [("value", Some(ty::int()))];
    let function = |index| b::lambda(param.clone(), b::add(b::name("value"), b::int(index)));
    b::list((0..count).map(function))
}

/// The most one more definition adds to a program: its code, its
/// hash and the name of its type, and an entry for each word it is
/// laid out in. Storing each of those words by code took a hundred
/// bytes more. Lower it when a definition takes less.
const DEFINITION: usize = 330;

#[test]
fn a_definition_is_laid_out_from_data() {
    let each = (size(functions(200), BARE) - size(functions(100), BARE)) / 100;
    assert!(each <= DEFINITION, "{each} bytes");
}

#[test]
#[ignore = "prints what the floor is made of"]
#[allow(clippy::print_stderr)]
fn what_the_floor_is_made_of() {
    let parts = program(b::null());
    let module = module_for(&parts, BARE).unwrap();
    eprintln!("{} bytes in all", module.len());
    // Somewhere to look at the module itself, if asked for.
    if let Some(path) = std::env::var_os("FOOTPRINT") {
        std::fs::write(path, &module).unwrap();
    }
    let mut pieces = pieces(&parts, BARE).unwrap();
    pieces.sort_by_key(|piece| std::cmp::Reverse(piece.size));
    for piece in pieces {
        eprintln!("{:>6} {}", piece.size, piece.name);
    }
}

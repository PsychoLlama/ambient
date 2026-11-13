//! Compiled parts as bytes, for a host that has no compiler. See
//! "Packs" in `spec/exchange.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::backend::native::{Host, Native, Natives, Program};
use ambient::backend::wasm::Wasm;
use ambient::builder::{self as b, ty};
use ambient::engine::{Backend, Engine, Parts, Unloadable};
use ambient::store::MemoryStore;
use ambient::value::{Value, decode};
use ambient_runtime::amb_release;
use proptest::prelude::*;
use support::value;

/// The bytes of two programs compiled natively: `main`, which gives a
/// string, and `plugin`, to load into it, which calls a function and
/// makes a cell at a type of its own.
fn compiled() -> (Vec<u8>, Vec<u8>, String) {
    let point = b::type_("Point", ty::record([("x", ty::int())]));
    let held = b::cell(ty::named("Point"), b::record([("x", b::int(20))]));
    let double = b::function("double")
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::add(b::name("n"), b::name("n")));
    let x = b::field(b::read(held), "x");
    let plugin = b::function("plugin")
        .returns(ty::int())
        .body(b::add(b::call(b::name("double"), [x]), b::int(2)));
    let main = b::function("main")
        .returns(ty::string())
        .body(b::string("made"));
    let workspace = support::workspace([
        ("app", "main", b::module().define(main)),
        (
            "app",
            "plugin",
            b::module().define(point).define(double).define(plugin),
        ),
    ]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let native = Native::new().unwrap();
    let main = engine.link_open(&value("main", "main"), &native).unwrap();
    let plugin = engine.link_to_load(&value("plugin", "plugin"), &native);
    (
        main.encode(&native),
        plugin.unwrap().encode(&native),
        native.name().to_owned(),
    )
}

/// What a function of a program gives, called with nothing.
fn given(program: &Program, function: Option<u64>) -> Value {
    let word = match function {
        Some(function) => program.call_value(function, &[]),
        None => program.call(&[]),
    }
    .unwrap();
    let value = decode(word, &mut Host);
    for word in function.into_iter().chain([word]) {
        // SAFETY: the word is owned here and not used again.
        unsafe { amb_release(word) };
    }
    value
}

#[test]
fn a_host_with_no_compiler_loads_and_runs_bytes() {
    let (main, plugin, name) = compiled();
    let natives = Natives::new();

    let program = Program::load(&Parts::decode(&main, &name).unwrap(), &natives).unwrap();
    let loaded = program.add(&Parts::decode(&plugin, &name).unwrap(), &natives);

    assert_eq!(given(&program, None), Value::String("made".to_owned()));
    assert_eq!(given(&program, Some(loaded.unwrap())), Value::Int(42));
}

#[test]
fn code_for_another_backend_is_refused() {
    let (main, _, name) = compiled();

    assert_eq!(
        Parts::decode(&main, Wasm.name()),
        Err(Unloadable::Backend(name))
    );
}

proptest! {
    #[test]
    fn bytes_that_are_not_parts_are_refused_not_trusted(
        cut in 0usize..100,
        flip in 0usize..200,
        bit in 0u8..8,
    ) {
        let (main, _, name) = compiled();
        let mut bytes = main[..main.len().saturating_sub(cut)].to_vec();
        let at = flip % bytes.len();
        bytes[at] ^= 1 << bit;

        // Whatever the bytes are now, reading them ends.
        let read = Parts::decode(&bytes, &name);
        prop_assert!(cut == 0 || read.is_err());
    }
}

//! What loaded code carries of the runtime. See "Loading" in
//! `spec/linking.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeSet;

use ambient::backend::native::{Native, Natives, Program};
use ambient::backend::wasm::{self, HOST, Wasm};
use ambient::builder::{self as b, pat, ty};
use ambient::engine::{Engine, Parts};
use ambient::hash::Hash;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr};
use ambient::value::{Value, decode};
use support::{value, workspace};

/// `fn plugin(): unknown`, with this body.
fn plugin(body: Expr) -> Definition {
    b::function("plugin").returns(ty::unknown()).body(body)
}

/// Joins two strings: an operation a program that does nothing lacks.
fn joins() -> Definition {
    plugin(b::join(b::string("ab"), b::string("cd")))
}

/// A program that does nothing, as a module code can be loaded into,
/// and each of `plugins` compiled to load, with what it would carry of
/// the runtime as a program of its own.
fn compiled<const N: usize>(plugins: [Definition; N]) -> (Vec<u8>, [(Parts, BTreeSet<Hash>); N]) {
    let main = b::function("main").returns(ty::unknown()).body(b::null());
    let mut workspace = workspace([("app", "main", b::module().define(main))]);
    let paths: [String; N] = std::array::from_fn(|index| format!("plugin{index}"));
    for (path, plugin) in paths.iter().zip(plugins) {
        let module = b::module().define(plugin).build().unwrap();
        workspace
            .set(ambient::symbol::ModuleId::new("app", path), module)
            .unwrap();
    }
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let main = engine.link_open(&value("main", "main"), &Wasm).unwrap();
    let parts = paths.map(|path| {
        let name = value(&path, "plugin");
        let alone = engine.link(&name, &Wasm).unwrap();
        let carried = wasm::pieces(&alone, HOST).unwrap();
        let carried = carried.iter().filter(|piece| piece.function);
        let parts = engine.link_to_load(&name, &Wasm).unwrap();
        (parts, carried.map(|piece| piece.hash).collect())
    });
    (wasm::open_module(&main).unwrap(), parts)
}

fn load(program: &[u8]) -> wasm::Program {
    wasm::Program::load(program, &Natives::new()).unwrap()
}

/// Loads a unit and calls its function with `args`.
fn run(program: &mut wasm::Program, unit: &[u8], args: &[u64]) -> Result<Value, String> {
    let function = program.add(unit, &Natives::new())?;
    let result = program.call_value(function, args).unwrap();
    Ok(decode(result, program.guest()))
}

fn joined() -> Value {
    Value::String("abcd".to_owned())
}

#[test]
fn loaded_code_brings_the_runtime_its_program_lacks() {
    let (program, [(parts, alone)]) = compiled([joins()]);
    let mut program = load(&program);
    let had = program.pieces();
    assert!(!alone.is_subset(&had));

    let unit = wasm::unit(&parts, &had).unwrap();

    assert_eq!(run(&mut program, &unit, &[]), Ok(joined()));
    assert!(alone.is_subset(&program.pieces()));
}

#[test]
fn code_told_of_nothing_loads_and_is_only_larger() {
    let (program, [(parts, _)]) = compiled([joins()]);
    let mut program = load(&program);
    let told = wasm::unit(&parts, &program.pieces()).unwrap();

    let untold = wasm::unit(&parts, &BTreeSet::new()).unwrap();

    assert!(untold.len() > told.len());
    assert_eq!(run(&mut program, &untold, &[]), Ok(joined()));
}

#[test]
fn code_told_of_what_its_target_lacks_does_not_load() {
    let (program, [(parts, alone)]) = compiled([joins()]);
    let mut program = load(&program);

    let unit = wasm::unit(&parts, &alone).unwrap();

    assert!(run(&mut program, &unit, &[]).is_err());
}

#[test]
fn what_one_load_brought_the_next_is_told_of() {
    let (program, [(parts, _)]) = compiled([joins()]);
    let mut program = load(&program);
    let first = wasm::unit(&parts, &program.pieces()).unwrap();
    assert_eq!(run(&mut program, &first, &[]), Ok(joined()));

    let second = wasm::unit(&parts, &program.pieces()).unwrap();

    assert!(second.len() < first.len());
    assert_eq!(run(&mut program, &second, &[]), Ok(joined()));
}

#[test]
fn code_loaded_apart_shares_the_state_of_its_program() {
    // A tag is the same tag only where it was interned once.
    let arms = [(pat::tag("found"), b::int(1)), (pat::any(), b::int(0))];
    let reads = b::function("plugin")
        .param("tag", ty::unknown())
        .returns(ty::unknown())
        .body(b::match_(b::name("tag"), arms));
    let (program, [(makes, _), (reads, _)]) = compiled([plugin(b::tag("found")), reads]);
    let mut program = load(&program);
    let nothing = BTreeSet::new();
    let makes = wasm::unit(&makes, &nothing).unwrap();
    let reads = wasm::unit(&reads, &nothing).unwrap();

    let made = program.add(&makes, &Natives::new()).unwrap();
    let tag = program.call_value(made, &[]).unwrap();

    assert_eq!(run(&mut program, &reads, &[tag]), Ok(Value::Int(1)));
}

#[test]
fn code_brings_the_state_its_target_lacks() {
    use wasmtime::{Engine, Linker, Module, Store, Val};
    let (program, [(parts, _)]) = compiled([joins()]);
    let unit = wasm::unit(&parts, &BTreeSet::new()).unwrap();
    // A host that shares the program's memory and says it has nothing
    // else: every state is at zero, which is to say nowhere.
    let engine = Engine::default();
    let mut store = Store::new(&engine, ());
    let mut linker = Linker::new(&engine);
    let program = Module::new(&engine, program).unwrap();
    let program = linker.instantiate(&mut store, &program).unwrap();
    linker.instance(&mut store, "rt", program).unwrap();
    let unit = Module::new(&engine, unit).unwrap();
    linker
        .define_unknown_imports_as_default_values(&mut store, &unit)
        .unwrap();
    let loaded = linker.instantiate(&mut store, &unit).unwrap();

    let init = loaded.get_func(&mut store, "init").unwrap();
    init.call(&mut store, &[], &mut [Val::I64(0)]).unwrap();

    let states = unit
        .exports()
        .filter(|export| export.name().starts_with("state:"));
    let names: Vec<&str> = states.map(|export| export.name()).collect();
    let mut address = |name: &&str| loaded.get_global(&mut store, name).unwrap().get(&mut store);
    let addresses: BTreeSet<i32> = names
        .iter()
        .map(|name| address(name).unwrap_i32())
        .collect();
    assert!(!names.is_empty());
    assert_eq!(addresses.len(), names.len());
    assert!(!addresses.contains(&0));
}

#[test]
fn nothing_loads_into_a_program_that_was_not_linked_for_it() {
    let main = b::function("main").returns(ty::unknown()).body(b::null());
    let workspace = workspace([("app", "main", b::module().define(main).define(joins()))]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let (main, plugin) = (value("main", "main"), value("main", "plugin"));
    let native = Native::new().unwrap();
    let closed = engine.link(&main, &native).unwrap();
    let program = Program::load(&closed, &Natives::new()).unwrap();
    let parts = engine.link_to_load(&plugin, &native).unwrap();

    assert!(program.add(&parts, &Natives::new()).is_err());
    assert!(wasm::open_module(&engine.link(&main, &Wasm).unwrap()).is_err());
}

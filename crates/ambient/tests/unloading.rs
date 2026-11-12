//! How long loaded code stays in a native program. See "Receiving" in
//! `spec/exchange.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::HashMap;

use ambient::backend::native::{Host, Native, Natives, Program};
use ambient::builder::{self as b, ModuleBuilder, ty};
use ambient::engine::{Engine, Parts};
use ambient::hash::Hash;
use ambient::store::MemoryStore;
use ambient::surface::Definition;
use ambient::value::{Value, decode};
use ambient_runtime::{Word, amb_int_new, amb_live, amb_release};
use support::value;

/// A function of no parameters that gives `body`.
fn giving(name: &str, body: ambient::surface::Expr) -> Definition {
    b::function(name).returns(ty::unknown()).body(body)
}

/// A program that does nothing, and functions to load into it: those
/// of a module it does not reach, by name.
struct Plugins {
    program: Program,
    parts: HashMap<String, (Parts, Hash)>,
    /// How many objects were live before anything was loaded.
    live: u64,
}

impl Plugins {
    fn new<const N: usize>(module: ModuleBuilder, names: [&str; N]) -> Self {
        let main = b::module().define(giving("main", b::null()));
        let workspace = support::workspace([("app", "main", main), ("app", "plugin", module)]);
        let store = MemoryStore::new();
        let engine = Engine::new(&workspace, &store);
        let native = Native::new().unwrap();
        let load = |name: &str| {
            let symbol = value("plugin", name);
            let parts = engine.link_to_load(&symbol, &native).unwrap();
            (name.to_owned(), (parts, engine.hash(&symbol).unwrap()))
        };
        let parts = names.into_iter().map(load).collect();
        let entry = engine.link(&value("main", "main"), &native).unwrap();
        let program = Program::load(&entry, &Natives::new()).unwrap();
        Self {
            program,
            parts,
            live: amb_live(),
        }
    }

    /// Loads a function, which the caller owns.
    fn add(&self, name: &str) -> Word {
        self.program
            .add(&self.parts[name].0, &Natives::new())
            .unwrap()
    }

    /// Whether the program has a function once what nothing refers to
    /// is dropped.
    fn has(&self, name: &str) -> bool {
        self.program.reclaim();
        self.program.has(&self.parts[name].1)
    }

    /// Loads a function, calls it, and lets go of it: what it gave is
    /// all that is left of the load.
    fn made_by(&self, name: &str) -> Word {
        let function = self.add(name);
        let made = self.program.call_value(function, &[]).unwrap();
        release(function);
        made
    }

    /// Whether everything made since the program was loaded is gone.
    fn clean(&self) -> bool {
        self.program.reclaim();
        amb_live() == self.live
    }
}

fn release(word: Word) {
    // SAFETY: the word is owned by the caller and not used again.
    unsafe { amb_release(word) };
}

#[test]
fn loaded_code_goes_with_the_function_it_was_loaded_for() {
    let plugins = Plugins::new(b::module().define(giving("f", b::string("made"))), ["f"]);

    let function = plugins.add("f");
    assert!(plugins.has("f"));
    release(function);

    assert!(!plugins.has("f"));
    assert!(plugins.clean());
}

#[test]
fn a_closure_holds_the_code_it_runs() {
    let adder = b::lambda([("n", Some(ty::int()))], b::add(b::name("n"), b::int(1)));
    let plugins = Plugins::new(b::module().define(giving("f", adder)), ["f"]);

    let closure = plugins.made_by("f");
    assert!(plugins.has("f"));
    // SAFETY: making an integer has no preconditions.
    let sum = plugins
        .program
        .call_value(closure, &[unsafe { amb_int_new(4) }]);
    release(closure);

    assert_eq!(decode(sum.unwrap(), &mut Host), Value::Int(5));
    assert!(!plugins.has("f"));
}

#[test]
fn a_cell_holds_the_type_it_was_made_at() {
    let point = b::type_("Point", ty::record([("x", ty::int())]));
    let made = b::cell(ty::named("Point"), b::record([("x", b::int(1))]));
    let module = b::module().define(point).define(giving("f", made));
    let plugins = Plugins::new(module, ["f"]);

    let cell = plugins.made_by("f");
    assert!(plugins.has("f"));
    release(cell);

    assert!(!plugins.has("f"));
    assert!(plugins.clean());
}

#[test]
fn a_cell_holds_the_type_arguments_it_was_made_under() {
    let make = b::function("make")
        .type_param("T")
        .param("x", ty::named("T"))
        .returns(ty::cell(ty::named("T")))
        .body(b::cell(ty::named("T"), b::name("x")));
    let cell = b::call(b::name("make"), [b::int(1)]);
    let module = b::module().define(make).define(giving("f", cell));
    let plugins = Plugins::new(module, ["f"]);

    let cell = plugins.made_by("f");
    assert!(plugins.has("f"));
    release(cell);

    assert!(!plugins.has("f"));
    assert!(plugins.clean());
}

#[test]
fn a_string_outlives_the_code_it_was_written_in() {
    let plugins = Plugins::new(b::module().define(giving("f", b::string("made"))), ["f"]);

    let string = plugins.made_by("f");
    assert!(!plugins.has("f"));

    assert_eq!(decode(string, &mut Host), Value::String("made".to_owned()));
    release(string);
    assert!(plugins.clean());
}

#[test]
fn loaded_code_holds_the_loaded_code_it_calls() {
    let inner = giving("inner", b::string("made"));
    let outer = giving("outer", b::call(b::name("inner"), []));
    let plugins = Plugins::new(b::module().define(inner).define(outer), ["inner", "outer"]);

    let (inner, outer) = (plugins.add("inner"), plugins.add("outer"));
    release(inner);
    assert!(plugins.has("inner"));
    release(outer);

    assert!(!plugins.has("inner") && !plugins.has("outer"));
    assert!(plugins.clean());
}

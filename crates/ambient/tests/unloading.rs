//! How long loaded code stays in a program, on every backend. See
//! "Receiving" in `spec/exchange.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::HashMap;
use std::rc::Rc;

use ambient::backend::native::{Host, Native, Natives, Program};
use ambient::backend::wasm;
use ambient::builder::{self as b, ModuleBuilder, ty};
use ambient::engine::{self, Backend, Engine, Parts};
use ambient::hash::Hash;
use ambient::store::MemoryStore;
use ambient::surface::Definition;
use ambient::value::{Construct, Value, decode};
use ambient_runtime::{amb_int_new, amb_live, amb_release};
use support::value;

/// A function of no parameters that gives `body`.
fn giving(name: &str, body: ambient::surface::Expr) -> Definition {
    b::function(name).returns(ty::unknown()).body(body)
}

/// A program that does nothing, running on one backend, and functions
/// to load into it: those of a module it does not reach, by name, or
/// as `f under t`, the program that runs `f` under the target `t`.
trait Plugins {
    /// Loads a function, which the caller owns.
    fn add(&mut self, name: &str) -> u64;
    /// Whether the program has a function once what nothing refers to
    /// is dropped.
    fn has(&mut self, name: &str) -> bool;
    /// Calls a function value with integers; the caller owns the result.
    fn call(&mut self, function: u64, args: &[i64]) -> u64;
    /// What a word is, giving it up.
    fn take(&mut self, word: u64) -> Value;
    /// Whether everything made since the program was loaded is gone.
    fn clean(&mut self) -> bool;

    fn release(&mut self, word: u64) {
        self.take(word);
    }

    /// Loads a function, calls it, and lets go of it: what it gave is
    /// all that is left of the load.
    fn made_by(&mut self, name: &str) -> u64 {
        let function = self.add(name);
        let made = self.call(function, &[]);
        self.release(function);
        made
    }
}

/// The program and what to load into it, for one backend: `Code` is
/// what that backend's host loads.
struct Loaded<Program, Code> {
    program: Program,
    parts: HashMap<String, (Code, Hash)>,
    /// How many objects were live before anything was loaded.
    live: u64,
    /// The native code each load is given.
    natives: Natives,
}

/// Compiles `main`, which does nothing, and each of `names` in the
/// module beside it, with one backend.
fn compiled<const N: usize>(
    module: ModuleBuilder,
    names: [&str; N],
    backend: &dyn Backend,
) -> (Parts, HashMap<String, (Parts, Hash)>) {
    let main = b::module().define(giving("main", b::null()));
    let workspace = support::workspace([("app", "main", main), ("app", "plugin", module)]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let load = |name: &str| {
        let symbol = match name.split_once(" under ") {
            Some((entry, target)) => engine
                .program(&value("plugin", entry), &value("plugin", target))
                .unwrap(),
            None => value("plugin", name),
        };
        let parts = engine.link_to_load(&symbol, backend).unwrap();
        (name.to_owned(), (parts, engine.hash(&symbol).unwrap()))
    };
    let parts = names.into_iter().map(load).collect();
    (
        engine.link_open(&value("main", "main"), backend).unwrap(),
        parts,
    )
}

type Natively = Loaded<Program, Parts>;
type InWasm = Loaded<wasm::Program, Vec<u8>>;

/// The same program and plugins on every backend.
fn plugins<const N: usize>(module: &ModuleBuilder, names: [&str; N]) -> Vec<Box<dyn Plugins>> {
    plugins_with(module, names, &Natives::new())
}

/// The same program and plugins on every backend, each plugin loaded
/// with `natives`.
fn plugins_with<const N: usize>(
    module: &ModuleBuilder,
    names: [&str; N],
    natives: &Natives,
) -> Vec<Box<dyn Plugins>> {
    let (main, parts) = compiled(module.clone(), names, &Native::new().unwrap());
    let program = Program::load(&main, &Natives::new()).unwrap();
    let native = Natively {
        program,
        parts,
        live: amb_live(),
        natives: natives.clone(),
    };
    let (main, parts) = compiled(module.clone(), names, &wasm::Wasm);
    let main = wasm::open_module(&main).unwrap();
    let mut program = wasm::Program::load(&main, &Natives::new()).unwrap();
    let (live, has) = (program.live(), program.pieces());
    let unit = |(name, (parts, hash)): (String, (Parts, Hash))| {
        (name, (wasm::unit(&parts, &has).unwrap(), hash))
    };
    let wasm = InWasm {
        program,
        parts: parts.into_iter().map(unit).collect(),
        live,
        natives: natives.clone(),
    };
    vec![Box::new(native), Box::new(wasm)]
}

impl Plugins for Natively {
    fn add(&mut self, name: &str) -> u64 {
        self.program
            .add(&self.parts[name].0, &self.natives)
            .unwrap()
    }

    fn has(&mut self, name: &str) -> bool {
        self.program.reclaim();
        self.program.has(&self.parts[name].1)
    }

    fn call(&mut self, function: u64, args: &[i64]) -> u64 {
        // SAFETY: making an integer has no preconditions.
        let args: Vec<u64> = args
            .iter()
            .map(|arg| unsafe { amb_int_new(*arg) })
            .collect();
        self.program.call_value(function, &args).unwrap()
    }

    fn take(&mut self, word: u64) -> Value {
        let value = decode(word, &mut Host);
        // SAFETY: the word is owned by the caller and not used again.
        unsafe { amb_release(word) };
        value
    }

    fn clean(&mut self) -> bool {
        self.program.reclaim();
        amb_live() == self.live
    }
}

impl Plugins for InWasm {
    fn add(&mut self, name: &str) -> u64 {
        self.program
            .add(&self.parts[name].0, &self.natives)
            .unwrap()
    }

    fn has(&mut self, name: &str) -> bool {
        self.program.reclaim();
        self.program.has(&self.parts[name].1)
    }

    fn call(&mut self, function: u64, args: &[i64]) -> u64 {
        let guest = self.program.guest();
        let args: Vec<u64> = args.iter().map(|arg| Construct::int(guest, *arg)).collect();
        self.program.call_value(function, &args).unwrap()
    }

    fn take(&mut self, word: u64) -> Value {
        let value = decode(word, self.program.guest());
        self.program.guest().release(word);
        value
    }

    fn clean(&mut self) -> bool {
        self.program.reclaim();
        self.program.live() == self.live
    }
}

#[test]
fn loaded_code_goes_with_the_function_it_was_loaded_for() {
    let module = b::module().define(giving("f", b::string("made")));
    for mut plugins in plugins(&module, ["f"]) {
        let function = plugins.add("f");
        assert!(plugins.has("f"));
        plugins.release(function);

        assert!(!plugins.has("f"));
        assert!(plugins.clean());
    }
}

#[test]
fn a_closure_holds_the_code_it_runs() {
    let adder = b::lambda([("n", Some(ty::int()))], b::add(b::name("n"), b::int(1)));
    let module = b::module().define(giving("f", adder));
    for mut plugins in plugins(&module, ["f"]) {
        let closure = plugins.made_by("f");
        assert!(plugins.has("f"));
        let sum = plugins.call(closure, &[4]);
        plugins.release(closure);

        assert_eq!(plugins.take(sum), Value::Int(5));
        assert!(!plugins.has("f"));
    }
}

#[test]
fn a_cell_holds_the_type_it_was_made_at() {
    let point = b::type_("Point", ty::record([("x", ty::int())]));
    let made = b::cell(ty::named("Point"), b::record([("x", b::int(1))]));
    let module = b::module().define(point).define(giving("f", made));
    for mut plugins in plugins(&module, ["f"]) {
        let cell = plugins.made_by("f");
        assert!(plugins.has("f"));
        plugins.release(cell);

        assert!(!plugins.has("f"));
        assert!(plugins.clean());
    }
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
    for mut plugins in plugins(&module, ["f"]) {
        let cell = plugins.made_by("f");
        assert!(plugins.has("f"));
        plugins.release(cell);

        assert!(!plugins.has("f"));
        assert!(plugins.clean());
    }
}

#[test]
fn a_string_outlives_the_code_it_was_written_in() {
    let module = b::module().define(giving("f", b::string("made")));
    for mut plugins in plugins(&module, ["f"]) {
        let string = plugins.made_by("f");
        assert!(!plugins.has("f"));

        assert_eq!(plugins.take(string), Value::String("made".to_owned()));
        assert!(plugins.clean());
    }
}

#[test]
fn loaded_code_holds_the_loaded_code_it_calls() {
    let inner = giving("inner", b::string("made"));
    let outer = giving("outer", b::call(b::name("inner"), []));
    let module = b::module().define(inner).define(outer);
    for mut plugins in plugins(&module, ["inner", "outer"]) {
        let (inner, outer) = (plugins.add("inner"), plugins.add("outer"));
        plugins.release(inner);
        assert!(plugins.has("inner"));
        plugins.release(outer);

        assert!(!plugins.has("inner") && !plugins.has("outer"));
        assert!(plugins.clean());
    }
}

#[test]
fn a_host_loads_a_program_with_a_target_of_its_own() {
    let ask: Definition = b::ability("ask").returns(ty::int()).into();
    let answer = b::function("answer").returns(ty::int()).body(b::int(42));
    let asks = b::function("f")
        .performs(["ask"])
        .returns(ty::int())
        .body(b::perform("ask", []));
    let world = b::target("world").bind("ask", "answer");
    let module = [ask, answer, asks, world.into()]
        .into_iter()
        .fold(b::module(), ModuleBuilder::define);
    for mut plugins in plugins(&module, ["f under world"]) {
        let answered = plugins.made_by("f under world");

        assert_eq!(plugins.take(answered), Value::Int(42));
        assert!(!plugins.has("f under world"));
    }
}

#[test]
fn a_function_named_as_a_value_holds_its_code() {
    let inner = giving("inner", b::string("made"));
    let module = b::module()
        .define(inner)
        .define(giving("f", b::name("inner")));
    for mut plugins in plugins(&module, ["f"]) {
        let named = plugins.made_by("f");
        assert!(plugins.has("f"));
        let made = plugins.call(named, &[]);
        plugins.release(named);

        assert_eq!(plugins.take(made), Value::String("made".to_owned()));
        assert!(plugins.clean());
    }
}

#[test]
fn native_code_goes_with_the_code_that_reaches_it() {
    let natives = Natives::new().with("app", "plugin.seven", |_| Ok(Value::Int(7)));
    let seven = engine::Native {
        package: "app".to_owned(),
        path: "plugin.seven".to_owned(),
    };
    // How many hold the native code: this list, and each load of it.
    let held = || Rc::strong_count(&natives.get(&seven).unwrap().run);
    let call = b::call(b::name("seven"), []);
    let module = b::module()
        .define(b::extern_function("seven").returns(ty::int()))
        .define(giving("f", call));
    for mut plugins in plugins_with(&module, ["f"], &natives) {
        let before = held();
        let function = plugins.add("f");
        assert_eq!(held(), before + 1);
        plugins.release(function);

        assert!(!plugins.has("f"));
        assert_eq!(held(), before);
    }
}

//! Loading code into a program that is running. See "Trust" and
//! "Receiving" in `spec/exchange.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::rc::Rc;

use ambient::backend::native::{Host, Native, Natives, Program};
use ambient::backend::wasm;
use ambient::builder::{self as b, pat, ty};
use ambient::engine::{Backend, Engine, Parts};
use ambient::hash::Hash;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr, Type};
use ambient::symbol::Symbol;
use ambient::value::{Construct, Value, decode};
use ambient_runtime::{Fault, amb_int_new, amb_release};
use support::value;

/// `fn plugin(n: int): int`, which doubles.
fn double() -> Definition {
    b::function("plugin")
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::add(b::name("n"), b::name("n")))
}

/// `fn plugin(n: int): int`, which calls itself `n` times.
fn count() -> Definition {
    let less = b::call(b::name("plugin"), [b::subtract(b::name("n"), b::int(1))]);
    let arms = [
        (pat::int(0), b::int(0)),
        (pat::any(), b::add(less, b::int(1))),
    ];
    b::function("plugin")
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::match_(b::name("n"), arms))
}

fn int_to_int() -> Type {
    ty::function([ty::int()], ty::int())
}

/// What `fetch()` gives, as a function of type `ty` called with `arg`,
/// or -1 if it is not one.
fn fetched(ty: Type, arg: i64) -> Expr {
    let arms = [
        (pat::is(ty::null()), b::int(-1)),
        (pat::bind("f"), b::call(b::name("f"), [b::int(arg)])),
    ];
    let cast = b::cast(b::call(b::name("fetch"), []), ty, b::null());
    b::match_(cast, arms)
}

/// A program whose `main` is `body`, on each backend, and the function
/// its host hands it when it calls `fetch`: one `find` names, loaded
/// then and not before.
struct Loading {
    native: Program,
    wasm: wasm::Program,
    /// The function to load, as the native backend compiled it.
    parts: Rc<Parts>,
    /// The hash of the function to load.
    plugin: Hash,
}

/// Native code for `fetch` that loads `parts` into whatever calls it.
fn fetching(parts: &Rc<Parts>) -> Natives {
    let parts = Rc::clone(parts);
    Natives::new().reaching("app", "main.fetch", move |program, _| {
        program.load(&parts, &Natives::new())
    })
}

impl Loading {
    fn new(plugin: Definition, body: Expr, find: impl FnOnce(&Engine) -> Symbol) -> Self {
        let fetch = b::extern_function("fetch").returns(ty::unknown());
        let main = b::function("main").returns(ty::unknown()).body(body);
        let workspace = support::workspace([
            ("app", "main", b::module().define(fetch).define(main)),
            ("app", "plugin", b::module().define(plugin)),
        ]);
        let store = MemoryStore::new();
        let engine = Engine::new(&workspace, &store);
        let (found, main) = (find(&engine), value("main", "main"));
        let compiled = |backend: &dyn Backend| {
            let plugin = Rc::new(engine.link_to_load(&found, backend).unwrap());
            (engine.link_open(&main, backend).unwrap(), plugin)
        };

        let (program, parts) = compiled(&Native::new().unwrap());
        let native = Program::load(&program, &fetching(&parts)).unwrap();
        let (program, unit) = compiled(&wasm::Wasm);
        let program = wasm::open_module(&program).unwrap();
        Self {
            native,
            wasm: wasm::Program::load(&program, &fetching(&unit)).unwrap(),
            parts,
            plugin: engine.hash(&found).unwrap(),
        }
    }

    /// With the function in the program's own workspace, in a module
    /// `main` does not reach.
    fn local(plugin: Definition, body: Expr) -> Self {
        Self::new(plugin, body, |_| value("plugin", "plugin"))
    }

    /// Whether either program has the function.
    fn has(&self) -> bool {
        self.native.has(&self.plugin) || self.wasm.has(&self.plugin)
    }

    /// Runs `main` on each backend, and gives what they agree on.
    fn run(&mut self) -> Result<Value, Fault> {
        let native = self.native.call(&[]).map(|word| {
            let value = decode(word, &mut Host);
            // SAFETY: the word is owned here and not used again.
            unsafe { amb_release(word) };
            value
        });
        let wasm = self.wasm.call(&[]).map(|word| {
            let value = decode(word, self.wasm.guest());
            self.wasm.guest().release(word);
            value
        });
        assert_eq!(native, wasm);
        native
    }
}

#[test]
fn a_function_loaded_while_a_program_runs_is_called() {
    let mut loading = Loading::local(double(), fetched(int_to_int(), 21));

    assert!(!loading.has());
    assert_eq!(loading.run(), Ok(Value::Int(42)));
    // Nothing the call left refers to it.
    assert!(!loading.has());
}

#[test]
fn a_loaded_function_of_another_type_does_not_pass_the_cast() {
    let other = ty::function([ty::int()], ty::string());

    assert_eq!(
        Loading::local(double(), fetched(other, 21)).run(),
        Ok(Value::Int(-1))
    );
}

#[test]
fn what_is_loaded_twice_is_there_once() {
    let fetch = || b::call(b::name("fetch"), []);
    let mut loading = Loading::local(double(), b::equal(fetch(), fetch()));

    assert_eq!(loading.run(), Ok(Value::Bool(true)));
}

#[test]
fn a_host_calls_a_function_it_loaded() {
    let loading = Loading::local(double(), b::null());
    let program = &loading.native;
    let function = program.add(&loading.parts, &Natives::new()).unwrap();

    // SAFETY: making an integer has no preconditions.
    let result = program.call_value(function, &[unsafe { amb_int_new(4) }]);

    assert_eq!(decode(result.unwrap(), &mut Host), Value::Int(8));
}

#[test]
fn a_function_that_arrived_in_a_pack_is_loaded_and_called() {
    let sender = support::workspace([("app", "main", b::module().define(double()))]);
    let sent = MemoryStore::new();
    let sending = Engine::new(&sender, &sent);
    let pack = sending.pack(&value("main", "plugin"), &|_| false).unwrap();

    // The receiver's own `plugin` is another function; what it loads is
    // the one the pack brought.
    let mut loading = Loading::new(count(), fetched(int_to_int(), 21), |engine| {
        engine.receive(&pack, &[]).unwrap()
    });

    assert_eq!(loading.run(), Ok(Value::Int(42)));
}

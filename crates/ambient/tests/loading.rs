//! Loading code into a program that is running. See "Trust" and
//! "Receiving" in `spec/exchange.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::OnceCell;
use std::rc::Rc;

use ambient::backend::native::{Host, Native, Natives, Program};
use ambient::builder::{self as b, pat, ty};
use ambient::engine::Engine;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr, Type};
use ambient::symbol::Symbol;
use ambient::value::{Function, Value, decode};
use ambient_runtime::{FAULT_PACKAGE, Fault, amb_int_new, amb_release};
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

/// A program whose `main` is `body`, and the function its host hands it
/// when it calls `fetch`: one `find` names, loaded then and not before.
struct Loading {
    program: Rc<OnceCell<Program>>,
    /// The parts of the function to load, and its hash.
    plugin: Rc<(ambient::engine::Parts, ambient::hash::Hash)>,
}

impl Loading {
    fn new(plugin: Definition, body: Expr, find: impl FnOnce(&Engine) -> Symbol) -> Self {
        let fetch = b::extern_function("fetch", "fetch").returns(ty::unknown());
        let main = b::function("main").returns(ty::unknown()).body(body);
        let workspace = support::workspace([
            ("app", "main", b::module().define(fetch).define(main)),
            ("app", "plugin", b::module().define(plugin)),
        ]);
        let store = MemoryStore::new();
        let engine = Engine::new(&workspace, &store);
        let native = Native::new().unwrap();
        let found = find(&engine);
        let parts = engine.link_to_load(&found, &native).unwrap();
        let plugin = Rc::new((parts, engine.hash(&found).unwrap()));

        let program = Rc::new(OnceCell::new());
        let (running, loaded) = (Rc::downgrade(&program), Rc::clone(&plugin));
        let natives = Natives::new().with("app", "fetch", move |_| {
            let running: Rc<OnceCell<Program>> = running.upgrade().ok_or("no program")?;
            let program = running.get().ok_or("no program")?;
            let function = program.add(&loaded.0, &Natives::new())?;
            // SAFETY: the function is owned here. What was loaded stays
            // until the next load, and by then the value returned is a
            // closure that holds it.
            unsafe { amb_release(function) };
            Ok(Value::Function(Function {
                definition: Some(loaded.1),
                captures: Vec::new(),
            }))
        });
        let parts = engine.link(&value("main", "main"), &native).unwrap();
        let loaded = Program::load(&parts, &natives).unwrap();
        assert!(program.set(loaded).is_ok());
        Self { program, plugin }
    }

    /// With the function in the program's own workspace, in a module
    /// `main` does not reach.
    fn local(plugin: Definition, body: Expr) -> Self {
        Self::new(plugin, body, |_| value("plugin", "plugin"))
    }

    fn program(&self) -> &Program {
        self.program.get().unwrap()
    }

    fn run(&self) -> Result<Value, Fault> {
        let word = self.program().call(&[])?;
        let value = decode(word, &mut Host);
        // SAFETY: the word is owned here and not used again.
        unsafe { amb_release(word) };
        Ok(value)
    }
}

#[test]
fn a_function_loaded_while_a_program_runs_is_called() {
    let loading = Loading::local(double(), fetched(int_to_int(), 21));

    assert!(!loading.program().has(&loading.plugin.1));
    assert_eq!(loading.run(), Ok(Value::Int(42)));
    // Nothing the call left refers to it.
    assert!(!loading.program().has(&loading.plugin.1));
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
fn a_loaded_function_is_metered_inside_a_budget() {
    let limit = |steps| {
        let body = b::budget(b::int(steps), b::int(1000), fetched(int_to_int(), 10));
        Loading::local(count(), body).run()
    };
    let steps = Value::Tag {
        package: FAULT_PACKAGE.to_owned(),
        tag: "steps".to_owned(),
    };

    assert_eq!(limit(100), Ok(Value::Int(10)));
    assert_eq!(limit(5), Ok(steps));
}

#[test]
fn what_is_loaded_twice_is_there_once() {
    let fetch = || b::call(b::name("fetch"), []);
    let loading = Loading::local(double(), b::equal(fetch(), fetch()));

    assert_eq!(loading.run(), Ok(Value::Bool(true)));
}

#[test]
fn a_host_calls_a_function_it_loaded() {
    let loading = Loading::local(double(), b::null());
    let program = loading.program();
    let function = program.add(&loading.plugin.0, &Natives::new()).unwrap();

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
    let loading = Loading::new(count(), fetched(int_to_int(), 21), |engine| {
        engine.receive(&pack, &[]).unwrap()
    });

    assert_eq!(loading.run(), Ok(Value::Int(42)));
}

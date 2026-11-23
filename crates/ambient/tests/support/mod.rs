//! Shared scaffolding for engine tests: a workspace in memory, an engine
//! over it, and a backend that records what it was asked to compile.

#![allow(dead_code, clippy::unwrap_used)]

pub mod delays;
pub mod packages;
pub mod scripted;
pub mod tasks;
pub mod text;

use std::cell::RefCell;

use ambient::backend::native::Natives;
use ambient::builder::ModuleBuilder;
use ambient::core::Open;
use ambient::engine::Backend;
use ambient::symbol::{ModuleId, Symbol};
use ambient::workspace::Workspace;

/// A workspace from `(package, module path, module)` triples.
pub fn workspace<const N: usize>(modules: [(&str, &str, ModuleBuilder); N]) -> Workspace {
    let mut workspace = Workspace::new();
    for (package, path, module) in modules {
        workspace
            .set(ModuleId::new(package, path), module.build().unwrap())
            .unwrap();
    }
    workspace
}

/// The function `name` in module `path` of package `app`.
pub fn value(path: &str, name: &str) -> Symbol {
    ModuleId::new("app", path).value(name)
}

/// The type `name` in module `path` of package `app`.
pub fn ty(path: &str, name: &str) -> Symbol {
    ModuleId::new("app", path).ty(name)
}

/// A backend whose artifact is the tree's encoding.
#[derive(Default)]
pub struct Recording {
    pub compiled: RefCell<usize>,
}

impl Backend for Recording {
    fn name(&self) -> &'static str {
        "recording"
    }

    fn compile(&self, definition: &Open, _form: &ambient::lower::Form) -> Result<Vec<u8>, String> {
        *self.compiled.borrow_mut() += 1;
        Ok(definition.tree.encode())
    }
}

/// What running a program gives: its result, or the fault that ended it.
pub type Outcome = Result<ambient::value::Value, ambient_runtime::Fault>;

/// Runs `main` of a one-module program with integer arguments, on every
/// backend, and returns what they agree on.
pub fn run(
    definitions: impl IntoIterator<Item = ambient::surface::Definition>,
    args: &[i64],
) -> Outcome {
    execute(definitions, args, None, Natives::new)
}

/// Runs `main` of a one-module program on every backend. With `target`,
/// it runs under that target, defined beside it. `natives` makes the
/// native code the program is given, once for each backend.
pub fn run_with(
    target: Option<&str>,
    definitions: impl IntoIterator<Item = ambient::surface::Definition>,
    natives: impl Fn() -> Natives,
) -> Outcome {
    execute(definitions, &[], target, natives)
}

fn execute(
    definitions: impl IntoIterator<Item = ambient::surface::Definition>,
    args: &[i64],
    target: Option<&str>,
    natives: impl Fn() -> Natives,
) -> Outcome {
    let workspace = program(definitions);
    execute_in(&workspace, args, target, natives)
}

/// A one-module program: module `main` of package `app`, beside the
/// standard library.
pub fn program(definitions: impl IntoIterator<Item = ambient::surface::Definition>) -> Workspace {
    let module = definitions
        .into_iter()
        .fold(ambient::builder::module(), ModuleBuilder::define)
        // What [`cleanup`] names its key through.
        .import("ambient.cleanup");
    let mut workspace = workspace([("app", "main", module)]);
    ambient::library::install(&mut workspace).unwrap();
    workspace.depend("app", "ambient", "ambient").unwrap();
    workspace
}

/// Runs `main` of module `main` of package `app` in a workspace, on every
/// backend, and returns what they agree on.
pub fn run_in(workspace: &Workspace) -> Outcome {
    execute_in(workspace, &[], None, Natives::new)
}

pub fn execute_in(
    workspace: &Workspace,
    args: &[i64],
    target: Option<&str>,
    natives: impl Fn() -> Natives,
) -> Outcome {
    use ambient::engine::Engine;
    use ambient::store::MemoryStore;

    let store = MemoryStore::new();
    let engine = Engine::new(workspace, &store);
    let main = value("main", "main");
    let entry = match target {
        Some(target) => engine.program(&main, &value("main", target)).unwrap(),
        None => main,
    };

    verify_all(&engine, &entry);
    run_entry(&engine, &entry, args, natives)
}

/// Runs `main` of a one-module program natively and nowhere else, under
/// the target `world` defined beside it: for native code no other
/// backend takes, as a native bundle's is.
pub fn run_natively(
    definitions: impl IntoIterator<Item = ambient::surface::Definition>,
    natives: &Natives,
) -> Outcome {
    use ambient::backend::native::Native;
    use ambient::engine::Engine;
    use ambient::store::MemoryStore;

    let workspace = program(definitions);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let entry = engine.program(&value("main", "main"), &value("main", "world"));
    let entry = entry.unwrap();
    verify_all(&engine, &entry);
    let parts = engine.link(&entry, &Native::new().unwrap()).unwrap();
    run_native(&parts, &[], natives).0
}

/// Links `entry` and runs it on every backend, and returns what they
/// agree on.
pub fn run_entry(
    engine: &ambient::engine::Engine,
    entry: &Symbol,
    args: &[i64],
    natives: impl Fn() -> Natives,
) -> Outcome {
    run_for_error(engine, entry, args, natives).0
}

/// [`run_entry`], with the error a fault that ended the program reached
/// its host as.
pub fn run_for_error(
    engine: &ambient::engine::Engine,
    entry: &Symbol,
    args: &[i64],
    natives: impl Fn() -> Natives,
) -> (Outcome, Option<ambient::value::Value>) {
    use ambient::backend::native::Native;
    use ambient::backend::wasm::Wasm;

    let parts = engine.link(entry, &Native::new().unwrap()).unwrap();
    let native = run_native(&parts, args, &natives());
    let parts = engine.link(entry, &Wasm).unwrap();
    let wasm = run_wasm(&parts, args, &natives());
    assert_eq!(native, wasm, "the backends disagree");
    native
}

/// Runs a program on every backend, as `link` compiles it for each and
/// with the native code it gives, and returns what they agree on.
pub fn run_linked(
    link: impl Fn(&dyn ambient::engine::Backend) -> (ambient::engine::Parts, Natives),
) -> Outcome {
    use ambient::backend::native::Native;
    use ambient::backend::wasm::Wasm;

    let (parts, natives) = link(&Native::new().unwrap());
    let (native, _) = run_native(&parts, &[], &natives);
    let (parts, natives) = link(&Wasm);
    let (wasm, _) = run_wasm(&parts, &[], &natives);
    assert_eq!(native, wasm, "the backends disagree");
    native
}

/// Verifies everything `entry` reaches as a receiver of it would: what
/// checking makes, the core checker accepts. Everything is checked
/// first, so that a definition that does not check is the one named.
pub fn verify_all(engine: &ambient::engine::Engine, entry: &Symbol) {
    let mut reached = std::collections::BTreeSet::new();
    let mut pending = vec![entry.clone()];
    while let Some(symbol) = pending.pop() {
        if !reached.insert(symbol.clone()) {
            continue;
        }
        let checked = engine.check(&symbol);
        let open = checked.unwrap_or_else(|error| panic!("a definition does not check: {error}"));
        pending.extend(open.slots.iter().cloned());
    }
    for symbol in &reached {
        if let Err(error) = engine.verify(symbol) {
            panic!("the core checker refuses what checking made: {error}");
        }
    }
}

/// An outcome, and the error a fault that ended the program was.
type Ended = (Outcome, Option<ambient::value::Value>);

fn run_native(parts: &ambient::engine::Parts, args: &[i64], natives: &Natives) -> Ended {
    use ambient::backend::native::{Host, Program};

    let program = Program::load(parts, natives).unwrap();
    let live = ambient_runtime::amb_live();
    // SAFETY: making an integer has no preconditions.
    let words: Vec<u64> = args
        .iter()
        .map(|arg| unsafe { ambient_runtime::amb_int_new(*arg) })
        .collect();
    let result = program.call(&words).map(|word| {
        let value = ambient::value::decode(word, &mut Host);
        // SAFETY: the word is owned by this call and not used again.
        unsafe { ambient_runtime::amb_release(word) };
        value
    });
    for word in words {
        // SAFETY: as above.
        unsafe { ambient_runtime::amb_release(word) };
    }
    assert_eq!(ambient_runtime::amb_live(), live, "the program leaked");
    (result, program.error())
}

fn run_wasm(parts: &ambient::engine::Parts, args: &[i64], natives: &Natives) -> Ended {
    use ambient::backend::wasm::{Program, module};
    use ambient::value::Construct;

    // A program code is loaded into is a module of another kind.
    let module = if parts.open {
        ambient::backend::wasm::open_module(parts)
    } else {
        module(parts)
    };
    let mut program = Program::load(&module.unwrap(), natives).unwrap();
    let live = program.live();
    let words: Vec<u64> = args
        .iter()
        .map(|arg| Construct::int(program.guest(), *arg))
        .collect();
    let result = program.call(&words).map(|word| {
        let value = ambient::value::decode(word, program.guest());
        program.guest().release(word);
        value
    });
    for word in words {
        program.guest().release(word);
    }
    assert_eq!(program.live(), live, "the program leaked");
    (result, program.error())
}

/// Evaluates `body`, and then `run`, however `body` ended: a using of a
/// record that holds `run` as its cleanup and nothing else.
pub fn cleanup(
    run: ambient::surface::Expr,
    body: ambient::surface::Expr,
) -> ambient::surface::Expr {
    use ambient::builder as b;
    let dropped = b::lambda([], b::let_("_", run, b::null()));
    let held = b::keyed(b::record([]), "cleanup.dispose", dropped);
    b::using("_", held, body)
}

/// `expr`, bound and then given: a call written so is not the last
/// thing its function does, so the function is still there, and in a
/// trace, while what it called runs.
pub fn kept(expr: ambient::surface::Expr) -> ambient::surface::Expr {
    ambient::builder::let_("kept", expr, ambient::builder::name("kept"))
}

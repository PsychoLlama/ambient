//! Shared scaffolding for engine tests: a workspace in memory, an engine
//! over it, and a backend that records what it was asked to compile.

#![allow(dead_code, clippy::unwrap_used)]

pub mod tasks;

use std::cell::RefCell;

use ambient::backend::native::{Natives, Offered};
use ambient::builder::ModuleBuilder;
use ambient::core::Open;
use ambient::engine::Backend;
use ambient::symbol::{ModuleId, Symbol};
use ambient::workspace::Workspace;
use wasmtime::{Caller, Linker, Module, Store, Val};

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

    fn compile(&self, definition: &Open, _metered: bool) -> Result<Vec<u8>, String> {
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
    let module = definitions
        .into_iter()
        .fold(ambient::builder::module(), ModuleBuilder::define);
    let workspace = workspace([("app", "main", module)]);
    execute_in(&workspace, args, target, natives)
}

/// Runs `main` of module `main` of package `app` in a workspace, on every
/// backend, and returns what they agree on.
pub fn run_in(workspace: &Workspace) -> Outcome {
    execute_in(workspace, &[], None, Natives::new)
}

fn execute_in(
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

/// Links `entry` and runs it on every backend, and returns what they
/// agree on.
pub fn run_entry(
    engine: &ambient::engine::Engine,
    entry: &Symbol,
    args: &[i64],
    natives: impl Fn() -> Natives,
) -> Outcome {
    use ambient::backend::native::Native;
    use ambient::backend::wasm::Wasm;

    let parts = engine.link(entry, &Native::new().unwrap()).unwrap();
    let native = run_native(&parts, args, &natives());
    let parts = engine.link(entry, &Wasm).unwrap();
    let wasm = run_wasm(&parts, args, &natives());
    assert_eq!(native, wasm, "the backends disagree");
    native
}

/// Verifies everything `entry` reaches as a receiver of it would: what
/// checking makes, the core checker accepts.
pub fn verify_all(engine: &ambient::engine::Engine, entry: &Symbol) {
    let mut seen = std::collections::BTreeSet::new();
    let mut pending = vec![entry.clone()];
    while let Some(symbol) = pending.pop() {
        if !seen.insert(symbol.clone()) {
            continue;
        }
        if let Err(error) = engine.verify(&symbol) {
            panic!("the core checker refuses what checking made: {error}");
        }
        pending.extend(engine.check(&symbol).unwrap().slots.iter().cloned());
    }
}

fn run_native(parts: &ambient::engine::Parts, args: &[i64], natives: &Natives) -> Outcome {
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
    result
}

/// What a WebAssembly host keeps: the runtime, once it is instantiated,
/// and the native code for each extern of the program.
struct State {
    runtime: Option<wasmtime::Instance>,
    natives: Vec<Offered>,
}

/// The runtime instantiated in a WebAssembly engine, reached through a
/// store or through the caller of a host function.
struct Guest<S> {
    store: S,
    runtime: wasmtime::Instance,
}

fn word(value: u64) -> Val {
    Val::I64(value.cast_signed())
}

impl<S: wasmtime::AsContextMut> Guest<S> {
    /// Calls a runtime export that takes and returns integers.
    fn runtime(&mut self, name: &str, args: &[Val]) -> Option<Val> {
        let function = self.runtime.get_func(&mut self.store, name).unwrap();
        let mut results = vec![Val::I64(0); function.ty(&self.store).results().len()];
        function.call(&mut self.store, args, &mut results).unwrap();
        results.into_iter().next()
    }

    /// Calls a runtime export that takes words and returns one.
    fn words(&mut self, name: &str, args: &[u64]) -> u64 {
        let args: Vec<Val> = args.iter().copied().map(word).collect();
        let result = self.runtime(name, &args);
        result.map_or(0, |result| result.unwrap_i64().cast_unsigned())
    }

    /// Calls a runtime export that returns a word.
    fn words_of(&mut self, name: &str, args: &[Val]) -> u64 {
        let result = self.runtime(name, args);
        result.map_or(0, |result| result.unwrap_i64().cast_unsigned())
    }

    /// Copies `bytes` into the runtime's memory and calls `name` with
    /// their address and length.
    fn with_bytes(&mut self, name: &str, bytes: &[u8]) -> u64 {
        let room = word(bytes.len().div_ceil(8).max(1) as u64);
        let address = self.runtime("amb_words", &[room]).unwrap();
        let memory = self.runtime.get_memory(&mut self.store, "memory").unwrap();
        let start = usize::try_from(address.unwrap_i32()).unwrap();
        memory.write(&mut self.store, start, bytes).unwrap();
        let result = self.runtime(name, &[address, word(bytes.len() as u64)]);
        result.unwrap().unwrap_i64().cast_unsigned()
    }
}

impl<S: wasmtime::AsContextMut> ambient::value::Inspect for Guest<S> {
    fn kind(&mut self, word: u64) -> u64 {
        self.words("amb_kind", &[word])
    }

    fn int(&mut self, word: u64) -> i64 {
        self.words("amb_int_value", &[word]).cast_signed()
    }

    fn float_bits(&mut self, word: u64) -> u64 {
        self.words("amb_float_bits", &[word])
    }

    fn length(&mut self, word: u64) -> u64 {
        self.words("amb_length", &[word])
    }

    fn word_at(&mut self, word: u64, index: u64) -> u64 {
        self.words("amb_word_at", &[word, index])
    }

    fn string(&mut self, value: u64) -> Vec<u8> {
        let length = usize::try_from(self.length(value)).unwrap();
        let pointer = self.runtime("amb_string_bytes", &[word(value)]);
        let start = usize::try_from(pointer.unwrap().unwrap_i32()).unwrap();
        let memory = self.runtime.get_memory(&mut self.store, "memory").unwrap();
        memory.data(&self.store)[start..start + length].to_vec()
    }
}

impl<S: wasmtime::AsContextMut> ambient::value::Construct for Guest<S> {
    fn int(&mut self, value: i64) -> u64 {
        self.words("amb_int_new", &[value.cast_unsigned()])
    }

    fn float(&mut self, bits: u64) -> u64 {
        self.words("amb_float_new", &[bits])
    }

    fn string(&mut self, bytes: &[u8]) -> u64 {
        self.with_bytes("amb_string_new", bytes)
    }

    fn key(&mut self, name: &[u8]) -> u64 {
        self.with_bytes("amb_intern", name)
    }

    fn tag(&mut self, text: &[u8]) -> u64 {
        self.with_bytes("amb_tag", text)
    }

    fn sequence(&mut self, list: bool, length: u64) -> u64 {
        self.words("amb_sequence_new", &[u64::from(list), length])
    }

    fn sequence_set(&mut self, sequence: u64, index: u64, value: u64) {
        self.words("amb_sequence_set", &[sequence, index, value]);
    }

    fn record(&mut self, length: u64) -> u64 {
        self.words("amb_record_new", &[length])
    }

    fn record_set(&mut self, record: u64, key: u64, value: u64) {
        self.words("amb_record_set", &[record, key, value]);
    }

    fn release(&mut self, word: u64) {
        self.words("amb_release", &[word]);
    }
}

/// The host's side of `amb_native`: runs the native code for the extern
/// at `index` and hands back its result, or reports a fault.
fn native(
    mut caller: Caller<'_, State>,
    context: i32,
    index: i64,
    _bindings: i64,
    args: i64,
) -> i64 {
    let runtime = caller.data().runtime.unwrap();
    let natives = std::mem::take(&mut caller.data_mut().natives);
    let mut guest = Guest {
        store: &mut caller,
        runtime,
    };
    let ambient::value::Value::Tuple(args) =
        ambient::value::decode(args.cast_unsigned(), &mut guest)
    else {
        panic!("arguments are a tuple");
    };
    let result = if let Ok(value) = (natives[usize::try_from(index).unwrap()].run)(&args) {
        ambient::value::encode(&value, &mut guest)
    } else {
        let code = word(ambient_runtime::Fault::Native as u64);
        guest.runtime("amb_fault", &[Val::I32(context), code]);
        ambient_runtime::UNWIND
    };
    caller.data_mut().natives = natives;
    result.cast_signed()
}

fn run_wasm(parts: &ambient::engine::Parts, args: &[i64], natives: &Natives) -> Outcome {
    use ambient::backend::wasm::{RUNTIME, RUNTIME_MODULE, module};

    let engine = wasmtime::Engine::default();
    let state = State {
        runtime: None,
        natives: natives.resolve(&parts.natives).unwrap(),
    };
    let mut store = Store::new(&engine, state);
    let mut linker = Linker::new(&engine);
    linker.func_wrap("host", "native", native).unwrap();
    let runtime = Module::new(&engine, RUNTIME).unwrap();
    let runtime = linker.instantiate(&mut store, &runtime).unwrap();
    store.data_mut().runtime = Some(runtime);
    linker
        .instance(&mut store, RUNTIME_MODULE, runtime)
        .unwrap();
    let program = Module::new(&engine, module(parts).unwrap()).unwrap();
    let program = linker.instantiate(&mut store, &program).unwrap();
    let mut guest = Guest { store, runtime };

    let mut closure = [Val::I64(0)];
    let init = program.get_func(&mut guest.store, "init").unwrap();
    init.call(&mut guest.store, &[], &mut closure).unwrap();
    let context = guest.runtime("amb_context_new", &[]).unwrap();
    let live = guest.words("amb_live", &[]);

    // The entry is handed no bindings.
    let mut all = vec![context, closure[0], word(ambient_runtime::NULL)];
    for arg in args {
        all.push(guest.runtime("amb_int_new", &[Val::I64(*arg)]).unwrap());
    }
    let entry = program.get_func(&mut guest.store, "entry").unwrap();
    let mut result = [Val::I64(0)];
    entry.call(&mut guest.store, &all, &mut result).unwrap();
    // Unwinding in this mode is the root task suspending: the others take
    // their turns, and the entry runs again to resume it.
    while result[0].unwrap_i64().cast_unsigned() == ambient_runtime::UNWIND
        && guest.words_of("amb_context_mode", &[context]) == ambient_runtime::SUSPENDING
    {
        guest.runtime("amb_schedule", &[context]);
        entry.call(&mut guest.store, &all, &mut result).unwrap();
    }
    let returned = result[0].unwrap_i64().cast_unsigned();

    let outcome = if returned == ambient_runtime::UNWIND {
        let code = guest.runtime("amb_context_fault", &[context]).unwrap();
        Err(ambient_runtime::Fault::from_code(
            code.unwrap_i64().cast_unsigned(),
        ))
    } else {
        let value = ambient::value::decode(returned, &mut guest);
        guest.runtime("amb_release", &[result[0]]);
        Ok(value)
    };
    for arg in &all[3..] {
        guest.runtime("amb_release", &[*arg]);
    }
    guest.runtime("amb_context_free", &[context]);
    assert_eq!(guest.words("amb_live", &[]), live, "the program leaked");
    outcome
}

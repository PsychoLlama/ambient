//! Shared scaffolding for engine tests: a workspace in memory, an engine
//! over it, and a backend that records what it was asked to compile.

#![allow(dead_code, clippy::unwrap_used)]

use std::cell::RefCell;

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

    fn compile(&self, definition: &Open) -> Result<Vec<u8>, String> {
        *self.compiled.borrow_mut() += 1;
        Ok(definition.tree.encode())
    }
}

/// Runs `main` of a one-module program with integer arguments, on every
/// backend, and returns what they agree on.
pub fn run(
    definitions: impl IntoIterator<Item = ambient::surface::Definition>,
    args: &[i64],
) -> Result<ambient::value::Value, ambient_runtime::Fault> {
    use ambient::backend::native::Native;
    use ambient::backend::wasm::Wasm;
    use ambient::engine::Engine;
    use ambient::store::MemoryStore;

    let module = definitions
        .into_iter()
        .fold(ambient::builder::module(), ModuleBuilder::define);
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let entry = value("main", "main");

    let native = run_native(&engine.link(&entry, &Native::new().unwrap()).unwrap(), args);
    let wasm = run_wasm(&engine.link(&entry, &Wasm).unwrap(), args);
    assert_eq!(native, wasm, "the backends disagree");
    native
}

type Outcome = Result<ambient::value::Value, ambient_runtime::Fault>;

fn run_native(parts: &ambient::engine::Parts, args: &[i64]) -> Outcome {
    use ambient::backend::native::{Host, Program};

    let program = Program::load(parts).unwrap();
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
    result
}

/// The runtime instantiated in a WebAssembly engine.
struct Guest {
    store: wasmtime::Store<()>,
    runtime: wasmtime::Instance,
}

impl Guest {
    /// Calls a runtime export that takes and returns integers.
    fn runtime(&mut self, name: &str, args: &[wasmtime::Val]) -> Option<wasmtime::Val> {
        let function = self.runtime.get_func(&mut self.store, name).unwrap();
        let mut results = vec![wasmtime::Val::I64(0); function.ty(&self.store).results().len()];
        function.call(&mut self.store, args, &mut results).unwrap();
        results.into_iter().next()
    }

    fn word(&mut self, name: &str, word: u64) -> u64 {
        let result = self.runtime(name, &[wasmtime::Val::I64(word.cast_signed())]);
        result.unwrap().unwrap_i64().cast_unsigned()
    }
}

impl ambient::value::Inspect for Guest {
    fn kind(&mut self, word: u64) -> u64 {
        self.word("amb_kind", word)
    }

    fn int(&mut self, word: u64) -> i64 {
        self.word("amb_int_value", word).cast_signed()
    }

    fn float_bits(&mut self, word: u64) -> u64 {
        self.word("amb_float_bits", word)
    }

    fn length(&mut self, word: u64) -> u64 {
        self.word("amb_length", word)
    }

    fn word_at(&mut self, word: u64, index: u64) -> u64 {
        let args = [word, index].map(|arg| wasmtime::Val::I64(arg.cast_signed()));
        self.runtime("amb_word_at", &args)
            .unwrap()
            .unwrap_i64()
            .cast_unsigned()
    }

    fn string(&mut self, word: u64) -> Vec<u8> {
        let length = usize::try_from(self.length(word)).unwrap();
        let pointer = self.runtime(
            "amb_string_bytes",
            &[wasmtime::Val::I64(word.cast_signed())],
        );
        let start = usize::try_from(pointer.unwrap().unwrap_i32()).unwrap();
        let memory = self.runtime.get_memory(&mut self.store, "memory").unwrap();
        memory.data(&self.store)[start..start + length].to_vec()
    }
}

fn run_wasm(parts: &ambient::engine::Parts, args: &[i64]) -> Outcome {
    use ambient::backend::wasm::{RUNTIME, RUNTIME_MODULE, module};
    use wasmtime::{Instance, Linker, Module, Store, Val};

    let engine = wasmtime::Engine::default();
    let mut store = Store::new(&engine, ());
    let runtime = Instance::new(&mut store, &Module::new(&engine, RUNTIME).unwrap(), &[]).unwrap();
    let mut linker = Linker::new(&engine);
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

    // The entry is handed no bindings.
    let mut all = vec![
        context,
        closure[0],
        Val::I64(ambient_runtime::NULL.cast_signed()),
    ];
    for arg in args {
        all.push(guest.runtime("amb_int_new", &[Val::I64(*arg)]).unwrap());
    }
    let entry = program.get_func(&mut guest.store, "entry").unwrap();
    let mut result = [Val::I64(0)];
    entry.call(&mut guest.store, &all, &mut result).unwrap();
    let word = result[0].unwrap_i64().cast_unsigned();

    if word == ambient_runtime::UNWIND {
        let code = guest.runtime("amb_context_fault", &[context]).unwrap();
        return Err(ambient_runtime::Fault::from_code(
            code.unwrap_i64().cast_unsigned(),
        ));
    }
    Ok(ambient::value::decode(word, &mut guest))
}

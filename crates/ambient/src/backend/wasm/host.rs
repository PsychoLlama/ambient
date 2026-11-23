//! A program running in a WebAssembly engine, and loading code into it.
//!
//! This is the host `docs/spec/backends.md` leaves to an embedder, written
//! for wasmtime: it instantiates the program, and each unit loaded after that as one more module over the same
//! memory and table. Any other host does the same with the same
//! modules; nothing here needs the compiler.

use std::collections::BTreeSet;

use ambient_runtime::{Fault, NULL, SUSPENDING, UNWIND};
use wasmtime::{Config, Engine, Linker, Module, Store, Val};

use super::RUNTIME_MODULE;
use super::guest::{Guest, State, native, word};
use super::units::externs;
use crate::backend::native::Natives;
use crate::hash::Hash;
use crate::value::{self, Value};
use ambient_host::stack;

/// A program instantiated in wasmtime, ready to call. More can be
/// loaded into one made by [`super::open_module`]: see [`Program::add`].
pub struct Program {
    /// The runtime, and in its store what loading keeps: native code
    /// reaches that too.
    guest: Guest<Store<State>>,
    /// The closure of the program's entry.
    entry: u64,
    /// The fault that ended the last call, as an error.
    error: Option<Value>,
}

impl Program {
    /// Instantiates a program put together by
    /// [`super::module`] or [`super::open_module`], with the native
    /// code for its externs.
    ///
    /// # Errors
    ///
    /// Fails if the bytes are not such a module, or `natives` lacks an
    /// extern the program reaches.
    pub fn load(module: &[u8], natives: &Natives) -> Result<Self, String> {
        let text = |error: wasmtime::Error| format!("{error:#}");
        // Calls are counted, so the stack is never what ends them.
        let mut config = Config::new();
        // The engine asks that a stack it would make hold that much,
        // though it makes none: nothing here is asynchronous.
        config.async_stack_size(2 * stack::ROOM);
        config.max_wasm_stack(stack::ROOM);
        let engine = Engine::new(&config).map_err(text)?;
        let mut store = Store::new(&engine, State::default());
        let mut linker = Linker::new(&engine);
        linker.func_wrap("host", "native", native).map_err(text)?;
        let module = Module::new(&engine, module).map_err(text)?;
        let instance = linker.instantiate(&mut store, &module).map_err(text)?;
        // The program is its own runtime; what is loaded into it
        // borrows that.
        let runtime = instance;
        store.data_mut().runtime = Some(runtime);
        linker
            .instance(&mut store, RUNTIME_MODULE, runtime)
            .map_err(text)?;
        let mut guest = Guest { store, runtime };
        // Where the runtime lists the units nothing refers to, in a
        // program that units are loaded into.
        let open = instance.get_func(&mut guest.store, "amb_unit_take");
        let unused = open.map_or(0, |_| guest.words("amb_words", &[1]));
        let externs = externs(&module, instance, &mut guest.store);
        let supplied = natives.resolve(&externs)?;
        let state = guest.store.data_mut();
        state.natives.add(&externs, supplied);
        state.waits = natives.waits();
        (state.unused, state.linker) = (unused, Some(linker));
        let (entry, _) = guest.init(&module, instance, false, &[])?;
        Ok(Self {
            guest,
            entry,
            error: None,
        })
    }

    /// Loads more into the program: a module put together by
    /// [`super::unit`]. Returns its function, a value the caller owns.
    ///
    /// What the program has already, by hash, the unit uses; the rest
    /// it brings. It is kept while a value refers to it, as
    /// [`crate::backend::native::Program::add`] keeps what it loads.
    ///
    /// # Errors
    ///
    /// Fails if the bytes are not such a module, or `natives` lacks an
    /// extern it reaches. Nothing is loaded then.
    pub fn add(&mut self, module: &[u8], natives: &Natives) -> Result<u64, String> {
        self.guest.add(module, natives)
    }

    /// The pieces of the runtime the program has, by hash: its own,
    /// and what was loaded into it since. A [`super::unit`] for this
    /// program can be told of these.
    pub fn pieces(&mut self) -> BTreeSet<Hash> {
        self.guest.pieces()
    }

    /// Drops what was loaded and nothing refers to any more, and
    /// whatever only that referred to. Every load begins with this and
    /// every call ends with it.
    pub fn reclaim(&mut self) {
        self.guest.reclaim();
    }

    /// Whether the program has a definition, by its hash. One that was
    /// loaded is there while a value refers to it.
    #[must_use]
    pub fn has(&self, hash: &Hash) -> bool {
        self.guest.store.data().known.contains_key(hash)
    }

    /// The runtime, for reading and making the program's values.
    pub fn guest(&mut self) -> &mut Guest<Store<State>> {
        &mut self.guest
    }

    /// How many objects are live in the runtime.
    pub fn live(&mut self) -> u64 {
        self.guest.words("amb_live", &[])
    }

    /// The fault that ended the last call, as the error a boundary
    /// would have made of it.
    #[must_use]
    pub fn error(&self) -> Option<Value> {
        self.error.clone()
    }

    /// Calls the entry with `args`, which it borrows. The result is a
    /// word the caller owns.
    ///
    /// # Errors
    ///
    /// Returns the fault if the program faulted.
    pub fn call(&mut self, args: &[u64]) -> Result<u64, Fault> {
        self.call_value(self.entry, args)
    }

    /// Calls a function value of this program with `args`, which it
    /// borrows, and with nothing bound.
    ///
    /// # Errors
    ///
    /// Returns the fault if the function faulted.
    ///
    /// # Panics
    ///
    /// Panics if `function` is not a function of as many parameters.
    pub fn call_value(&mut self, function: u64, args: &[u64]) -> Result<u64, Fault> {
        stack::roomy(|| self.started(function, args))
    }

    /// Calls a function value on the stack it is running on.
    fn started(&mut self, function: u64, args: &[u64]) -> Result<u64, Fault> {
        let guest = &mut self.guest;
        let kind = guest.words("amb_kind", &[function]);
        assert_eq!(
            kind,
            ambient_runtime::Kind::Function as u64,
            "not a function"
        );
        let arity = guest.words("amb_closure_arity", &[function]);
        assert_eq!(args.len() as u64 + 1, arity, "wrong number of arguments");
        let context = guest.runtime("amb_context_new", &[]).unwrap_or(Val::I32(0));
        // The function is handed no bindings: nothing is provided to it.
        let handed = guest.words("amb_sequence_new", &[0, arity]);
        for (index, arg) in (1..).zip(args) {
            guest.runtime("amb_sequence_set", &[word(handed), word(index), word(*arg)]);
        }
        let start = [context, word(function), word(handed)];
        let run = |guest: &mut Guest<Store<State>>| {
            let result = guest.runtime("amb_start", &start);
            result.map_or(UNWIND, |result| result.unwrap_i64().cast_unsigned())
        };
        // Unwinding in this mode is the root task suspending: the others
        // take their turns, and the function runs again to resume it.
        let mut returned = run(guest);
        while returned == UNWIND
            && guest
                .runtime("amb_context_mode", &[context])
                .map(|mode| mode.unwrap_i64())
                == Some(SUSPENDING.cast_signed())
        {
            guest.schedule(u64::from(context.unwrap_i32().cast_unsigned()));
            returned = run(guest);
        }
        guest.words("amb_release", &[handed]);
        self.error = None;
        let outcome = if returned == UNWIND {
            let code = guest.runtime("amb_context_fault", &[context]);
            let error = guest.runtime("amb_context_error", &[context]);
            let error = error.map_or(NULL, |error| error.unwrap_i64().cast_unsigned());
            if error != NULL {
                self.error = Some(value::decode(error, guest));
                guest.words("amb_release", &[error]);
            }
            Err(Fault::from_code(
                code.map_or(0, |code| code.unwrap_i64().cast_unsigned()),
            ))
        } else {
            Ok(returned)
        };
        guest.runtime("amb_context_free", &[context]);
        guest.heard();
        guest.reclaim();
        outcome
    }
}

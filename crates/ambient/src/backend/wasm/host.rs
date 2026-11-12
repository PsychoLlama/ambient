//! A program running in a WebAssembly engine, and loading code into it.
//!
//! This is the host `spec/backends.md` leaves to an embedder, written
//! for wasmtime: it instantiates the runtime, then the program over it,
//! and each unit loaded after that as one more module over the same
//! memory and table. Any other host does the same with the same
//! modules; nothing here needs the compiler.

use ambient_runtime::{Fault, NULL, SUSPENDING, UNWIND};
use wasmtime::{Engine, Linker, Module, Store, Val};

use super::guest::{Guest, State, must, native, word};
use super::units::externs;
use super::{RUNTIME, RUNTIME_MODULE};
use crate::backend::native::Natives;
use crate::hash::Hash;
use crate::value::{self, Value};

/// A program instantiated in wasmtime, ready to call. More can be
/// loaded into one made by [`super::open_module`]: see [`Program::add`].
pub struct Program {
    /// The runtime, and in its store what loading keeps: native code
    /// reaches that too.
    guest: Guest<Store<State>>,
    /// The closure of the program's entry.
    entry: u64,
    /// A word of the runtime's memory to be handed results in.
    scratch: u64,
    /// The fault that ended the last call, as an error.
    error: Option<Value>,
}

impl Program {
    /// Instantiates the runtime and, over it, a program put together by
    /// [`super::module`] or [`super::open_module`], with the native
    /// code for its externs.
    ///
    /// # Errors
    ///
    /// Fails if the bytes are not such a module, or `natives` lacks an
    /// extern the program reaches.
    pub fn load(module: &[u8], natives: &Natives) -> Result<Self, String> {
        let text = |error: wasmtime::Error| error.to_string();
        let engine = Engine::default();
        let mut store = Store::new(&engine, State::default());
        let mut linker = Linker::new(&engine);
        linker.func_wrap("host", "native", native).map_err(text)?;
        let runtime = Module::new(&engine, RUNTIME).map_err(text)?;
        let runtime = linker.instantiate(&mut store, &runtime).map_err(text)?;
        store.data_mut().runtime = Some(runtime);
        linker
            .instance(&mut store, RUNTIME_MODULE, runtime)
            .map_err(text)?;

        let module = Module::new(&engine, module).map_err(text)?;
        let instance = linker.instantiate(&mut store, &module).map_err(text)?;
        let mut guest = Guest { store, runtime };
        let unused = guest.words("amb_words", &[1]);
        let scratch = guest.words("amb_words", &[1]);
        let externs = externs(&module, instance, &mut guest.store);
        let supplied = natives.resolve(&externs)?;
        let state = guest.store.data_mut();
        state.natives.add(&externs, supplied);
        (state.unused, state.linker) = (unused, Some(linker));
        let (entry, _) = guest.init(&module, instance, false, &[])?;
        Ok(Self {
            guest,
            entry,
            scratch,
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
        let guest = &mut self.guest;
        let kind = guest.words("amb_kind", &[function]);
        assert_eq!(
            kind,
            ambient_runtime::Kind::Function as u64,
            "not a function"
        );
        let scratch = Val::I32(u32::try_from(self.scratch).unwrap_or(0).cast_signed());
        let code = guest.runtime("amb_closure_code", &[word(function), scratch]);
        let code = code.map_or(0, |code| code.unwrap_i64().cast_unsigned());
        let arity = guest.read(self.scratch);
        assert_eq!(args.len() as u64 + 1, arity, "wrong number of arguments");
        let Some(table) = guest
            .runtime
            .get_table(&mut guest.store, "__indirect_function_table")
        else {
            panic!("the runtime has no table");
        };
        let code = table.get(&mut guest.store, code);
        let Some(code) = code.as_ref().and_then(|code| code.unwrap_func()).copied() else {
            panic!("a function's code is not in the table");
        };

        let context = guest.runtime("amb_context_new", &[]).unwrap_or(Val::I32(0));
        // The function is handed no bindings: nothing is provided to it.
        let mut all = vec![context, word(function), word(NULL)];
        all.extend(args.iter().copied().map(word));
        let mut result = [Val::I64(0)];
        let mut run = |guest: &mut Guest<Store<State>>| {
            must(code.call(&mut guest.store, &all, &mut result), "a call");
            result[0].unwrap_i64().cast_unsigned()
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
            guest.runtime("amb_schedule", &[context]);
            returned = run(guest);
        }
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
        guest.reclaim();
        outcome
    }
}

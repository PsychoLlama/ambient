//! A program running in a WebAssembly engine, and loading code into it.
//!
//! This is the host `spec/backends.md` leaves to an embedder, written
//! for wasmtime: it instantiates the runtime, then the program over it,
//! and each unit loaded after that as one more module over the same
//! memory and table. Any other host does the same with the same
//! modules; nothing here needs the compiler.

use std::collections::HashMap;

use ambient_runtime::{Fault, NULL, SUSPENDING, UNWIND};
use wasmtime::{Engine, Global, GlobalType, Instance, Linker, Module, Mutability, Store, Val};

use super::guest::{Guest, State, must, native, word};
use super::{HOST_MODULE, KNOWN_MODULE, RUNTIME, RUNTIME_MODULE};
use crate::backend::native::Natives;
use crate::engine::Native;
use crate::hash::Hash;
use crate::value::{self, Value};

/// Where the program has a definition.
#[derive(Clone, Copy)]
struct Known {
    root: u64,
    definition: u64,
    /// The unit it is in, or zero for the program's own.
    unit: u64,
    metered: bool,
}

/// What one load brought.
struct Unit {
    instance: Instance,
    /// The units it refers into, each a reference this holds.
    holds: Vec<u64>,
    /// The definitions it brought.
    hashes: Vec<Hash>,
}

/// A program instantiated in wasmtime, ready to call. More can be
/// loaded into one made by [`super::open_module`]: see [`Program::add`].
pub struct Program {
    guest: Guest<Store<State>>,
    linker: Linker<State>,
    /// The closure of the program's entry.
    entry: u64,
    known: HashMap<Hash, Known>,
    units: HashMap<u64, Unit>,
    /// The address of the word the runtime keeps the units nothing
    /// refers to under.
    unused: u64,
    /// A word of the runtime's memory to be handed results in.
    scratch: u64,
    /// The fault that ended the last call, as an error.
    error: Option<Value>,
}

/// What a module exports under names that begin `prefix`: the rest of
/// each name, and the value of the global.
fn exported(
    module: &Module,
    instance: Instance,
    store: &mut Store<State>,
    prefix: &str,
) -> Vec<(String, u64)> {
    let names = module.exports().filter_map(|export| {
        let name = export.name().strip_prefix(prefix)?;
        Some(name.to_owned())
    });
    names
        .collect::<Vec<_>>()
        .into_iter()
        .filter_map(|name| {
            let global = instance.get_global(&mut *store, &format!("{prefix}{name}"))?;
            match global.get(&mut *store) {
                Val::I64(value) => Some((name, value.cast_unsigned())),
                Val::I32(value) => Some((name, u64::from(value.cast_unsigned()))),
                _ => None,
            }
        })
        .collect()
}

/// The externs a module reaches, in the order compiled code numbers
/// them.
fn externs(module: &Module, instance: Instance, store: &mut Store<State>) -> Vec<Native> {
    let mut found = exported(module, instance, store, "native:");
    found.sort_by_key(|(_, position)| *position);
    let native = |(name, _): (String, u64)| {
        let (package, tag) = name.split_once('\t')?;
        Some(Native {
            package: package.to_owned(),
            tag: tag.to_owned(),
        })
    };
    found.into_iter().filter_map(native).collect()
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
        let mut program = Self {
            guest,
            linker,
            entry: NULL,
            known: HashMap::new(),
            units: HashMap::new(),
            unused,
            scratch,
            error: None,
        };
        let supplied = natives.resolve(&externs(&module, instance, &mut program.guest.store))?;
        program.guest.store.data_mut().natives = supplied;
        (program.entry, _) = program.init(&module, instance, false, &[])?;
        Ok(program)
    }

    /// Runs a module's `init`, and notes where it put each definition
    /// that is not among `had`: those its host said the program has.
    /// Returns the entry, and the address of the module's block, or
    /// zero for a module that does not say.
    fn init(
        &mut self,
        module: &Module,
        instance: Instance,
        loaded: bool,
        had: &[Hash],
    ) -> Result<(u64, u64), String> {
        let store = &mut self.guest.store;
        let init = instance
            .get_func(&mut *store, "init")
            .ok_or("the module has no `init`")?;
        let mut entry = [Val::I64(0)];
        init.call(&mut *store, &[], &mut entry)
            .map_err(|error| error.to_string())?;
        let entry = entry[0].unwrap_i64().cast_unsigned();
        // Nothing can be loaded into a program that does not say where
        // it is.
        let Some(base) = instance.get_global(&mut *store, "base") else {
            return Ok((entry, 0));
        };
        let base = u64::from(base.get(&mut *store).unwrap_i32().cast_unsigned());
        let metered = exported(module, instance, store, "metered:");
        let definitions: HashMap<String, u64> = exported(module, instance, store, "definition:")
            .into_iter()
            .collect();
        for (name, root) in exported(module, instance, store, "root:") {
            let Some(hash) = Hash::parse(&name).filter(|hash| !had.contains(hash)) else {
                continue;
            };
            let definition = definitions.get(&name).copied().filter(|at| *at != 0);
            let known = Known {
                root: base + root,
                definition: definition.map_or(0, |at| base + at),
                unit: if loaded { base } else { 0 },
                metered: metered.iter().any(|(other, _)| *other == name),
            };
            self.known.insert(hash, known);
        }
        Ok((entry, base))
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
        self.reclaim();
        let text = |error: wasmtime::Error| error.to_string();
        let module = Module::new(self.guest.store.engine(), module).map_err(text)?;
        let wanted: Vec<&str> = module.exports().map(|export| export.name()).collect();
        let wants_metered = |hash: &Hash| wanted.contains(&format!("metered:{hash}").as_str());

        let mut linker = self.linker.clone();
        let constant = GlobalType::new(wasmtime::ValType::I64, Mutability::Const);
        let mut define = |store: &mut Store<State>, module: &str, name: &str, value: u64| {
            let global = Global::new(&mut *store, constant.clone(), word(value)).map_err(text)?;
            linker
                .define(&mut *store, module, name, global)
                .map_err(text)?;
            Ok::<(), String>(())
        };
        let had = self.guest.store.data().natives.len() as u64;
        define(&mut self.guest.store, HOST_MODULE, "unused", self.unused)?;
        define(&mut self.guest.store, HOST_MODULE, "natives", had)?;
        let (mut holds, mut hashes) = (Vec::new(), Vec::new());
        for import in module
            .imports()
            .filter(|import| import.module() == KNOWN_MODULE)
        {
            let Some((what, hash)) = import.name().split_once(':') else {
                continue;
            };
            let hash = Hash::parse(hash).ok_or("a unit imports what is not a hash")?;
            // A function that is here without the metered form the unit
            // calls it by is brought again, with one.
            let known = self.known.get(&hash).copied();
            let known = known.filter(|known| known.metered || !wants_metered(&hash));
            let address = known.map_or(0, |known| match what {
                "root" => known.root,
                _ => known.definition,
            });
            define(&mut self.guest.store, KNOWN_MODULE, import.name(), address)?;
            if let Some(known) = known {
                hashes.push(hash);
                if known.unit != 0 && !holds.contains(&known.unit) {
                    holds.push(known.unit);
                }
            }
        }

        let instance = linker
            .instantiate(&mut self.guest.store, &module)
            .map_err(text)?;
        let supplied = natives.resolve(&externs(&module, instance, &mut self.guest.store))?;
        self.guest.store.data_mut().natives.extend(supplied);
        for unit in &holds {
            self.guest.words("amb_unit_retain", &[*unit]);
        }
        let (entry, base) = self.init(&module, instance, true, &hashes)?;
        let brought = self.known.iter().filter(|(_, known)| known.unit == base);
        let unit = Unit {
            instance,
            holds,
            hashes: brought.map(|(hash, _)| *hash).collect(),
        };
        self.units.insert(base, unit);
        Ok(entry)
    }

    /// Drops what was loaded and nothing refers to any more, and
    /// whatever only that referred to. Every load begins with this and
    /// every call ends with it.
    pub fn reclaim(&mut self) {
        loop {
            let unit = self.guest.at("amb_unit_take", self.unused, &[]);
            let Some(gone) = self.units.remove(&unit) else {
                break;
            };
            for hash in &gone.hashes {
                // A later unit may have brought the definition again.
                if self.known.get(hash).is_some_and(|known| known.unit == unit) {
                    self.known.remove(hash);
                }
            }
            if let Some(drop) = gone.instance.get_func(&mut self.guest.store, "drop") {
                must(drop.call(&mut self.guest.store, &[], &mut []), "drop");
            }
            for held in gone.holds {
                self.guest.words("amb_unit_release", &[held]);
            }
        }
    }

    /// Whether the program has a definition, by its hash. One that was
    /// loaded is there while a value refers to it.
    #[must_use]
    pub fn has(&self, hash: &Hash) -> bool {
        self.known.contains_key(hash)
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
        self.reclaim();
        outcome
    }
}

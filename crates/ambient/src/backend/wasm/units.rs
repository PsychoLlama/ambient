//! Loading code into a WebAssembly program that is running, and
//! dropping it again. See "Loading" in `docs/spec/backends.md`.
//!
//! All of it works through a [`Guest`], so that it is the same whether
//! the host loads between calls or native code loads in the middle of
//! one.

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;

use wasmtime::{AsContextMut, Global, GlobalType, Instance, Linker, Module, Mutability, Val};

use super::guest::{Guest, State, must, word};
use super::link::{PIECE_MODULE, STATE_MODULE};
use super::{HOST_MODULE, KNOWN_MODULE};
use crate::backend::native::Natives;
use crate::engine::Native;
use crate::hash::Hash;

/// Where the program has a definition.
#[derive(Clone, Copy)]
pub(super) struct Known {
    root: u64,
    pub(super) definition: u64,
    /// The unit it is in, or zero for the program's own.
    unit: u64,
}

/// What one load brought.
pub(super) struct Unit {
    instance: Instance,
    /// The units it refers into, each a reference this holds.
    holds: Vec<u64>,
    /// The definitions it brought.
    hashes: Vec<Hash>,
    /// Which of the program's externs it brought.
    natives: Range<usize>,
}

/// What a module exports under names that begin `prefix`: the rest of
/// each name, and the value of the global.
fn exported(
    module: &Module,
    instance: Instance,
    mut store: impl AsContextMut,
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
            let global = instance.get_global(&mut store, &format!("{prefix}{name}"))?;
            match global.get(&mut store) {
                Val::I64(value) => Some((name, value.cast_unsigned())),
                Val::I32(value) => Some((name, u64::from(value.cast_unsigned()))),
                _ => None,
            }
        })
        .collect()
}

/// The externs a module reaches, in the order compiled code numbers
/// them.
pub(super) fn externs(
    module: &Module,
    instance: Instance,
    store: impl AsContextMut,
) -> Vec<Native> {
    let mut found = exported(module, instance, store, "native:");
    found.sort_by_key(|(_, position)| *position);
    let native = |(name, _): (String, u64)| {
        let mut texts = name.split('\t').map(str::to_owned);
        let (package, path) = (texts.next()?, texts.next()?);
        let mut row = Vec::new();
        while let Some(package) = texts.next() {
            row.push((package, texts.next()?));
        }
        Some(Native { package, path, row })
    };
    found.into_iter().filter_map(native).collect()
}

impl<S: AsContextMut<Data = State>> Guest<S> {
    /// The pieces of the runtime the program has, by hash: what a
    /// [`super::unit`] for it can be told of.
    pub fn pieces(&mut self) -> BTreeSet<Hash> {
        self.state(|state| state.pieces.keys().copied().collect())
    }

    /// Notes what a module that was just laid out has of the runtime
    /// that the program lacked. It is the program's from then on.
    fn brought(&mut self, module: &Module, instance: Instance) {
        for (name, address) in exported(module, instance, &mut self.store, "state:") {
            if let Some(hash) = Hash::parse(&name) {
                self.state(|state| {
                    state.states.entry(hash).or_insert(address);
                });
            }
        }
        let names = module.exports().map(|export| export.name());
        for name in names.filter_map(|name| name.strip_prefix("piece:")) {
            let function = instance.get_func(&mut self.store, &format!("piece:{name}"));
            if let (Some(hash), Some(function)) = (Hash::parse(name), function) {
                self.state(|state| {
                    state.pieces.entry(hash).or_insert(function);
                });
            }
        }
    }

    /// Defines what a unit was told the program has of the runtime: a
    /// function by hash, and the address of a state, or zero for one
    /// the program lacks, which the unit then brings.
    fn tell(&mut self, module: &Module, linker: &mut Linker<State>) -> Result<(), String> {
        let text = |error: wasmtime::Error| format!("{error:#}");
        for import in module.imports() {
            let (name, hash) = (import.name(), Hash::parse(import.name()));
            match (import.module(), hash) {
                (PIECE_MODULE, Some(hash)) => {
                    let piece = self.state(|state| state.pieces.get(&hash).copied());
                    let piece = piece.ok_or("a unit was told of a piece the program lacks")?;
                    linker
                        .define(&mut self.store, PIECE_MODULE, name, piece)
                        .map_err(text)?;
                }
                (STATE_MODULE, Some(hash)) => {
                    let address = self.state(|state| state.states.get(&hash).copied());
                    let address = i32::try_from(address.unwrap_or(0)).unwrap_or(0);
                    let ty = GlobalType::new(wasmtime::ValType::I32, Mutability::Const);
                    let global =
                        Global::new(&mut self.store, ty, Val::I32(address)).map_err(text)?;
                    linker
                        .define(&mut self.store, STATE_MODULE, name, global)
                        .map_err(text)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Runs a module's `init`, and notes where it put each definition
    /// that is not among `had`: those its host said the program has.
    /// Returns the entry, and the address of the module's block, or
    /// zero for a module that does not say.
    pub(super) fn init(
        &mut self,
        module: &Module,
        instance: Instance,
        loaded: bool,
        had: &[Hash],
    ) -> Result<(u64, u64), String> {
        let store = &mut self.store;
        let init = instance
            .get_func(&mut *store, "init")
            .ok_or("the module has no `init`")?;
        let mut entry = [Val::I64(0)];
        init.call(&mut *store, &[], &mut entry)
            .map_err(|error| error.to_string())?;
        let entry = entry[0].unwrap_i64().cast_unsigned();
        self.brought(module, instance);
        let store = &mut self.store;
        // Nothing can be loaded into a program that does not say where
        // it is.
        let Some(base) = instance.get_global(&mut *store, "base") else {
            return Ok((entry, 0));
        };
        let base = u64::from(base.get(&mut *store).unwrap_i32().cast_unsigned());
        let definitions: HashMap<String, u64> =
            exported(module, instance, &mut *store, "definition:")
                .into_iter()
                .collect();
        for (name, root) in exported(module, instance, &mut *store, "root:") {
            let Some(hash) = Hash::parse(&name).filter(|hash| !had.contains(hash)) else {
                continue;
            };
            let definition = definitions.get(&name).copied().filter(|at| *at != 0);
            let known = Known {
                root: base + root,
                definition: definition.map_or(0, |at| base + at),
                unit: if loaded { base } else { 0 },
            };
            self.state(|state| state.known.insert(hash, known));
        }
        Ok((entry, base))
    }

    /// Loads a module put together by [`super::unit`]: see
    /// [`super::Program::add`].
    pub(super) fn add(&mut self, module: &[u8], natives: &Natives) -> Result<u64, String> {
        self.reclaim();
        let text = |error: wasmtime::Error| format!("{error:#}");
        let engine = self.store.as_context().engine().clone();
        let module = Module::new(&engine, module).map_err(text)?;

        let linker = self.state(|state| state.linker.clone());
        let mut linker = linker.ok_or("the program is not loaded yet")?;
        self.tell(&module, &mut linker)?;
        let constant = GlobalType::new(wasmtime::ValType::I64, Mutability::Const);
        let mut define = |store: &mut S, module: &str, name: &str, value: u64| {
            let global = Global::new(&mut *store, constant.clone(), word(value)).map_err(text)?;
            linker
                .define(&mut *store, module, name, global)
                .map_err(text)?;
            Ok::<(), String>(())
        };
        let (unused, had) = self.state(|state| (state.unused, state.natives.next() as u64));
        define(&mut self.store, HOST_MODULE, "unused", unused)?;
        define(&mut self.store, HOST_MODULE, "natives", had)?;
        let (mut holds, mut hashes) = (Vec::new(), Vec::new());
        for import in module
            .imports()
            .filter(|import| import.module() == KNOWN_MODULE)
        {
            let Some((what, hash)) = import.name().split_once(':') else {
                continue;
            };
            let hash = Hash::parse(hash).ok_or("a unit imports what is not a hash")?;
            let known = self.state(|state| state.known.get(&hash).copied());
            let address = known.map_or(0, |known| match what {
                "root" => known.root,
                _ => known.definition,
            });
            define(&mut self.store, KNOWN_MODULE, import.name(), address)?;
            if let Some(known) = known {
                hashes.push(hash);
                if known.unit != 0 && !holds.contains(&known.unit) {
                    holds.push(known.unit);
                }
            }
        }

        let instance = linker.instantiate(&mut self.store, &module).map_err(text)?;
        let externs = externs(&module, instance, &mut self.store);
        let supplied = natives.resolve(&externs)?;
        let natives = self.state(|state| state.natives.add(&externs, supplied));
        for unit in &holds {
            self.words("amb_unit_retain", &[*unit]);
        }
        let (entry, base) = self.init(&module, instance, true, &hashes)?;
        self.state(|state| {
            let brought = state.known.iter().filter(|(_, known)| known.unit == base);
            let unit = Unit {
                instance,
                holds,
                hashes: brought.map(|(hash, _)| *hash).collect(),
                natives,
            };
            state.units.insert(base, unit);
        });
        Ok(entry)
    }

    /// Drops what was loaded and nothing refers to any more: see
    /// [`super::Program::reclaim`].
    pub(super) fn reclaim(&mut self) {
        // A program nothing is loaded into has nothing to drop, and
        // not the part of the runtime that says what to.
        if self.state(|state| state.unused) == 0 {
            return;
        }
        loop {
            let unused = self.state(|state| state.unused);
            let unit = self.at("amb_unit_take", unused, &[]);
            let Some(gone) = self.state(|state| state.units.remove(&unit)) else {
                break;
            };
            self.state(|state| {
                state.natives.remove(gone.natives.clone());
                // A later unit may have brought a definition again.
                let known = &mut state.known;
                let here = |hash: &&Hash| known.get(*hash).is_some_and(|known| known.unit == unit);
                for hash in gone.hashes.iter().filter(here).collect::<Vec<_>>() {
                    known.remove(hash);
                }
            });
            if let Some(drop) = gone.instance.get_func(&mut self.store, "drop") {
                must(drop.call(&mut self.store, &[], &mut []), "drop");
            }
            for held in gone.holds {
                self.words("amb_unit_release", &[held]);
            }
        }
    }
}

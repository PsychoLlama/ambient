//! The runtime instantiated in a WebAssembly engine, and how a host
//! reads and makes values in it.

use std::collections::HashMap;

use wasmtime::{AsContextMut, Caller, Instance, Linker, Val};

use super::units::{Known, Unit};
use crate::backend::native::{Externs, Natives, Running};
use crate::engine::Parts;
use crate::hash::Hash;
use crate::value::{self, Construct, Inspect, Value};

/// What a WebAssembly host keeps where native code can reach it: the
/// runtime, once it is instantiated, the native code for each extern of
/// the program, and what loading into the program takes.
#[derive(Default)]
pub struct State {
    pub(super) runtime: Option<Instance>,
    pub(super) natives: Externs,
    /// What every module of the program is linked against.
    pub(super) linker: Option<Linker<State>>,
    pub(super) known: HashMap<Hash, Known>,
    pub(super) units: HashMap<u64, Unit>,
    /// The address of the word the runtime keeps the units nothing
    /// refers to under.
    pub(super) unused: u64,
}

/// The runtime, reached through a store or through the caller of a
/// host function.
pub struct Guest<S> {
    pub(super) store: S,
    pub(super) runtime: Instance,
}

/// What a trap in the runtime is to a host: the end, as a crash in
/// native code would be.
pub(super) fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

pub(super) fn word(value: u64) -> Val {
    Val::I64(value.cast_signed())
}

fn as_word(value: Option<Val>) -> u64 {
    match value {
        Some(Val::I64(word)) => word.cast_unsigned(),
        Some(Val::I32(address)) => u64::from(address.cast_unsigned()),
        _ => 0,
    }
}

impl<S: AsContextMut<Data = State>> Guest<S> {
    /// Reads or changes what the host keeps.
    pub(super) fn state<R>(&mut self, work: impl FnOnce(&mut State) -> R) -> R {
        work(self.store.as_context_mut().data_mut())
    }

    /// Calls a runtime export.
    pub(super) fn runtime(&mut self, name: &str, args: &[Val]) -> Option<Val> {
        let Some(function) = self.runtime.get_func(&mut self.store, name) else {
            panic!("the runtime has no `{name}`");
        };
        let mut results = vec![Val::I64(0); function.ty(&self.store).results().len()];
        must(function.call(&mut self.store, args, &mut results), name);
        results.into_iter().next()
    }

    /// Calls a runtime export that takes words, and returns a word or
    /// an address.
    pub(super) fn words(&mut self, name: &str, args: &[u64]) -> u64 {
        let args: Vec<Val> = args.iter().copied().map(word).collect();
        as_word(self.runtime(name, &args))
    }

    /// Calls a runtime export that takes an address first.
    pub(super) fn at(&mut self, name: &str, address: u64, args: &[u64]) -> u64 {
        let mut all = vec![Val::I32(u32::try_from(address).unwrap_or(0).cast_signed())];
        all.extend(args.iter().copied().map(word));
        as_word(self.runtime(name, &all))
    }

    /// The runtime's memory.
    fn memory(&mut self) -> wasmtime::Memory {
        match self.runtime.get_memory(&mut self.store, "memory") {
            Some(memory) => memory,
            None => panic!("the runtime has no memory"),
        }
    }

    /// Copies `bytes` into the runtime's memory and calls `name` with
    /// their address and length.
    fn with_bytes(&mut self, name: &str, bytes: &[u8]) -> u64 {
        let room = bytes.len().div_ceil(8).max(1) as u64;
        let address = self.words("amb_words", &[room]);
        let memory = self.memory();
        let start = usize::try_from(address).unwrap_or(usize::MAX);
        must(
            memory.write(&mut self.store, start, bytes),
            "writing memory",
        );
        let result = self.at(name, address, &[bytes.len() as u64]);
        self.at("amb_words_free", address, &[room]);
        result
    }
}

impl<S: AsContextMut<Data = State>> Inspect for Guest<S> {
    fn kind(&mut self, word: u64) -> u64 {
        self.words("amb_kind", &[word])
    }

    fn int(&mut self, word: u64) -> i64 {
        self.words("amb_int_value", &[word]).cast_signed()
    }

    fn float_bits(&mut self, word: u64) -> u64 {
        self.words("amb_float_bits", &[word])
    }

    fn definition(&mut self, word: u64) -> u64 {
        self.words("amb_closure_hash", &[word])
    }

    fn length(&mut self, word: u64) -> u64 {
        self.words("amb_length", &[word])
    }

    fn word_at(&mut self, word: u64, index: u64) -> u64 {
        self.words("amb_word_at", &[word, index])
    }

    fn string(&mut self, value: u64) -> Vec<u8> {
        let length = usize::try_from(self.length(value)).unwrap_or(0);
        let start = usize::try_from(self.words("amb_string_bytes", &[value])).unwrap_or(0);
        let memory = self.memory();
        memory.data(&self.store)[start..start + length].to_vec()
    }
}

impl<S: AsContextMut<Data = State>> Construct for Guest<S> {
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

    fn function(&mut self, definition: &Hash, captures: u64) -> Option<u64> {
        let known = self.state(|state| state.known.get(definition).copied())?;
        let definition = Some(known.definition).filter(|at| *at != 0)?;
        Some(self.words("amb_closure_new", &[definition, captures]))
    }

    fn capture(&mut self, function: u64, index: u64, value: u64) {
        self.words("amb_closure_set", &[function, index, value]);
    }

    fn method(&mut self, function: u64) {
        self.words("amb_method_mark", &[function]);
    }
}

/// One call of native code: the program it was made from, and the
/// functions it loaded, each held until what it returns is made.
struct Call<'a, 'b> {
    guest: Guest<&'a mut Caller<'b, State>>,
    held: Vec<u64>,
}

impl Running for Call<'_, '_> {
    fn load(&mut self, parts: &Parts, natives: &Natives) -> Result<Value, String> {
        let function = self.guest.add(&super::unit(parts)?, natives)?;
        self.held.push(function);
        parts.function()
    }
}

/// The host's side of `amb_native`: runs the native code for the extern
/// at `index` and hands back its result, or reports a fault.
pub(super) fn native(
    mut caller: Caller<'_, State>,
    context: i32,
    index: i64,
    _bindings: i64,
    args: i64,
) -> i64 {
    let Some(runtime) = caller.data().runtime else {
        panic!("native code ran before the runtime was there");
    };
    // Native code may load more into the program, so nothing of the
    // program is borrowed while it runs.
    let offered = usize::try_from(index)
        .ok()
        .and_then(|index| caller.data().natives.get(index).cloned());
    let offered = offered.map(|(_, offered)| offered);
    let guest = Guest {
        store: &mut caller,
        runtime,
    };
    let mut call = Call {
        guest,
        held: Vec::new(),
    };
    let decoded = value::decode(args.cast_unsigned(), &mut call.guest);
    let args: &[Value] = match &decoded {
        Value::Tuple(args) => args,
        _ => &[],
    };
    let outcome = offered.map(|offered| (offered.run)(&mut call, args));
    let Call { mut guest, held } = call;
    let result = if let Some(Ok(value)) = outcome {
        value::encode(&value, &mut guest)
    } else {
        let code = word(ambient_runtime::Fault::Native as u64);
        guest.runtime("amb_fault", &[Val::I32(context), code]);
        ambient_runtime::UNWIND
    };
    for function in held {
        guest.words("amb_release", &[function]);
    }
    result.cast_signed()
}

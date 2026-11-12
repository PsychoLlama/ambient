//! The WebAssembly backend.
//!
//! A program is two modules: the [runtime](RUNTIME), and one emitted by
//! [`module`] that imports the runtime's memory and entry points. Compiled
//! functions never refer to each other by index, only through closures in
//! memory, so each function's body is compiled on its own and a module is
//! put together by concatenation.

mod emit;
mod module;

use ambient_runtime::Entry;
pub use emit::Wasm;
pub use module::module;

/// The runtime as a WebAssembly module. It exports its memory and every
/// entry point; instantiate it first and give its exports to the program's
/// module under the name [`RUNTIME_MODULE`].
pub const RUNTIME: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ambient_runtime.wasm"));

/// The module name a program imports the runtime under.
pub const RUNTIME_MODULE: &str = "rt";

/// The function index of an entry point: they are the first imports.
pub(super) fn import(entry: Entry) -> u32 {
    entry as u32
}

/// The function index of `amb_words`, imported after the entry points.
// There are a handful of entry points; this cannot truncate.
#[allow(clippy::cast_possible_truncation)]
pub(super) const WORDS: u32 = Entry::ALL.len() as u32;

/// The function indices of the imports that make constants.
pub(super) const STRING_PINNED: u32 = WORDS + 1;
pub(super) const INTERN: u32 = WORDS + 2;

/// The type index of a compiled function taking `arity` arguments.
pub(super) fn function_type(arity: usize) -> u32 {
    u32::try_from(arity).unwrap_or(0)
}

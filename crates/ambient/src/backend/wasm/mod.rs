//! The WebAssembly backend.
//!
//! A program is two modules: the [runtime](RUNTIME), and one emitted by
//! [`module`] that imports the runtime's memory and entry points. Compiled
//! functions never refer to each other by index, only through closures in
//! memory, so each function's body is compiled on its own and a module is
//! put together by concatenation.
//!
//! Code is loaded into a program that is running as one more module, a
//! [`unit`], instantiated over the same memory and table. What the
//! program has already reaches it as imports, by hash, so the module is
//! the same bytes whatever program loads it.

mod emit;
mod guest;
mod host;
mod layout;
mod module;
mod resume;

use ambient_runtime::Entry;
pub use emit::Wasm;
pub use guest::{Guest, State};
pub use host::Program;
pub use module::{HOST_MODULE, KNOWN_MODULE, module, open_module, unit};

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
pub(super) const TAG: u32 = WORDS + 3;
/// The function indices of the imports only a unit calls: to make a
/// string it holds a reference to, to give back its block, and to hold
/// and let go of itself.
pub(super) const STRING_NEW: u32 = WORDS + 4;
pub(super) const WORDS_FREE: u32 = WORDS + 5;
pub(super) const UNIT_RETAIN: u32 = WORDS + 6;
pub(super) const UNIT_RELEASE: u32 = WORDS + 7;
/// The first function a module defines: every import comes before it.
pub(super) const FIRST: u32 = WORDS + 8;

/// The type index of a compiled function taking `arity` arguments.
pub(super) fn function_type(arity: usize) -> u32 {
    u32::try_from(arity).unwrap_or(0)
}

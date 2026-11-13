//! The WebAssembly backend.
//!
//! A program is one module, emitted by [`module`]: its compiled
//! functions, and the pieces of the runtime they reach (`link`).
//! Compiled functions never refer to each other by index, only through
//! closures in memory, so each function's body is compiled on its own
//! and a module is put together by concatenation.
//!
//! Code is loaded into a program that is running as one more module, a
//! [`unit`], instantiated over the same memory and table. What the
//! program has already reaches it as imports, by hash, so the module is
//! the same bytes whatever program loads it.

mod code;
mod emit;
mod guest;
mod host;
mod layout;
mod link;
mod module;
mod resume;
mod runtime;
mod units;

pub use emit::Wasm;
pub use guest::{Guest, State};
pub use host::Program;
pub use module::{HOST, HOST_MODULE, KNOWN_MODULE, module, module_for, open_module, pieces, unit};

/// The module name a unit imports its program under: the program's
/// memory, table, and stack, and the runtime's functions.
pub const RUNTIME_MODULE: &str = "rt";

/// The type index of a compiled function taking `arity` arguments.
pub(super) fn function_type(arity: usize) -> u32 {
    u32::try_from(arity).unwrap_or(0)
}

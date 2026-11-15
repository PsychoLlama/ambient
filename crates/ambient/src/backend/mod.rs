//! The backends. See `spec/backends.md`.
//!
//! Each translates the [lowered form](crate::lir) one construct at a time.
//! Both follow the same calling convention and the same layouts, which the
//! runtime crate defines:
//!
//! - a function takes the context, the closure being called, and then its
//!   arguments, and returns one word;
//! - it reaches other definitions through the slots of its closure's
//!   definition, and the runtime through entry points.

mod identity;
pub mod native;
pub(crate) mod stack;
pub mod statics;
pub mod wasm;

/// Why nothing is loaded into a program that was not linked for it.
pub(crate) const CLOSED: &str =
    "the program was not linked to be loaded into: see `Engine::link_open`";

/// The most parameters a function may take.
///
/// TODO: a limit only because the runtime calls a function of each
/// arity by a signature it names; nothing else depends on it.
pub const MAX_ARITY: usize = ambient_runtime::ARGUMENTS;

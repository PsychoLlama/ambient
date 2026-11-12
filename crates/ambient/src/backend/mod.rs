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

pub mod native;
pub mod statics;
pub mod wasm;

/// The most parameters a function may take.
///
/// TODO: a limit only because each arity needs a signature the host can
/// name; nothing else depends on it.
pub const MAX_ARITY: usize = 16;

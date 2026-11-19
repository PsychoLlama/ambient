//! The backends. See `docs/spec/backends.md`.
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
pub mod wasm;

use crate::engine::Backend;

pub(crate) use ambient_host::parts::CLOSED;
pub(crate) use ambient_host::stack;
pub use ambient_host::statics;

/// The most parameters a function may take.
///
/// TODO: a limit only because the runtime calls a function of each
/// arity by a signature it names; nothing else depends on it.
pub const MAX_ARITY: usize = ambient_runtime::ARGUMENTS;

/// The backend `name` names (`Backend::name`): what compiles code for
/// a program that says which it was compiled by (`Running::backend`).
///
/// # Errors
///
/// Fails if no backend here has the name, or the one that has cannot
/// compile for this machine.
pub fn named(name: &str) -> Result<Box<dyn Backend>, String> {
    if name == native::NAME {
        Ok(Box::new(native::Native::new()?))
    } else if name == wasm::NAME {
        Ok(Box::new(wasm::Wasm))
    } else {
        Err(format!("nothing here compiles for {name}"))
    }
}

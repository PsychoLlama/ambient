//! What `system.code` binds that needs no compiler: loading what was
//! compiled. See "Code" in `docs/spec/targets.md`.
//!
//! Packing a function and compiling one need the compiler, which adds
//! them (`ambient::host`).

use super::{Failure, Kind, PACKAGE, bytes, error};
use crate::native::{Natives, Running};
use crate::parts::Parts;
use crate::value::Value;

/// What a handler was given, refused as something it cannot take.
#[must_use]
pub fn invalid(message: &dyn ToString) -> Failure {
    Failure {
        kind: Kind::Invalid,
        message: message.to_string(),
    }
}

/// Loads what `code.compile` gave ([`Parts::encode`]) into the program
/// that asked, with `natives` for its externs. The bytes are trusted,
/// as compiled code is.
fn load(running: &mut dyn Running, natives: &Natives, compiled: &[u8]) -> Result<Value, Failure> {
    let backend = running.backend().map_err(|error| invalid(&error))?;
    let parts = Parts::decode(compiled, backend).map_err(|error| invalid(&error))?;
    running
        .load(&parts, natives)
        .map_err(|error| invalid(&error))
}

/// `natives`, with loading. What is loaded is given no native code but
/// what brings answers: a received definition names no handler.
pub(super) fn offer(natives: Natives) -> Natives {
    let base = natives
        .waits()
        .map_or_else(Natives::new, |waits| Natives::new().answered_by(waits));
    natives.reaching(PACKAGE, "code_load", move |running, args| {
        let done = load(running, &base, bytes(args, 0)?);
        Ok(done.unwrap_or_else(|failure| error("code", &failure)))
    })
}

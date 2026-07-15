//! Natives for `core::exception` (`0x0A__` block).

use crate::{Value, VmError};

use super::{NativeRegistry, arg, bind, module};

pub(super) fn register(reg: &mut NativeRegistry) {
    let exception = module(&["core", "exception"]);
    bind(reg, &exception, "uncaught", 0x0A01, 1, uncaught);
}

/// Deliver an uncaught throw to the host.
///
/// `Exception::throw`'s default implementation — the body an unhandled
/// perform runs — is this native's only caller (the extern is
/// module-private). Returning on the [`VmError::Exception`] channel makes
/// `finish_native_call` re-raise at the call site; no handler can be in
/// scope there (a covering handler would have fired instead of the default
/// implementation), so the raise falls straight through to the driving
/// host as an uncaught exception carrying the thrown value.
fn uncaught(mut args: Vec<Value>) -> Result<Value, VmError> {
    Err(VmError::Exception(arg(&mut args, 0)))
}

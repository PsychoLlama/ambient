//! Host handler types.

use crate::error::VmError;
use crate::value::{SuspendedAbility, Value};

/// A host handler for an ability method.
///
/// Host handlers are called when the VM executes an ability operation.
/// They receive the suspended ability (containing the method ID and arguments)
/// and return either a value or an error.
///
/// # Errors
///
/// Handlers have two failure channels:
///
/// - `Err(VmError::Exception(value))` raises a *catchable* language-level
///   exception at the perform site — the VM performs `Exception.throw(value)`
///   there, so the calling program's nearest `handle` block for Exception
///   catches it (catch-only: Exception arms cannot resume). Use
///   [`VmError::exception`] for the common string-message case. This is for
///   hard faults — an unwired capability, a runtime control error — *not*
///   fallible operations: those (file not found, connection refused, ...)
///   return an in-language `Result::Err` value instead.
/// - Any other `VmError` is a fatal engine error that aborts execution.
///
/// # Example
///
/// ```ignore
/// let print_handler: HostHandler = Box::new(|ability| {
///     let message = ability.args.first().cloned().unwrap_or(Value::Unit);
///     println!("{}", format_value(&message));
///     Ok(Value::Unit)
/// });
/// ```
pub type HostHandler = Box<dyn Fn(&SuspendedAbility) -> Result<Value, VmError> + Send + Sync>;

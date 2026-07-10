//! Core types for authoring Ambient abilities.
//!
//! This crate provides the fundamental types needed to implement custom abilities:
//!
//! - [`Value`] - Runtime values that abilities work with
//! - [`SuspendedAbility`] - Represents a suspended ability operation
//! - [`VmError`] - Errors that abilities can return
//!
//! # Example
//!
//! ```ignore
//! use ambient_ability::{format_value, Value};
//!
//! let rendered = format_value(&Value::string("hello"));
//! assert_eq!(rendered, "\"hello\"");
//! ```

#![warn(clippy::print_stdout, clippy::print_stderr)]
#![deny(
    clippy::pedantic,
    clippy::perf,
    clippy::style,
    clippy::complexity,
    clippy::correctness,
    clippy::suspicious
)]
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod error;
mod format;
mod method;
mod value;

pub use error::{RuntimeError, StackTraceFrame, VmError};
pub use format::{format_value, format_value_colored, format_value_display};
pub use method::{AbilityMethodRef, Closure, HandlerValue, SuspendedAbility};
pub use value::{
    CapturedFrame, CapturedHandler, Continuation, EnumValue, MapValue, ModuleExport,
    ModuleExportKind, ModuleMemberRef, ModuleValue, SetValue, Value,
};

// Re-export commonly used types from ambient-core
pub use ambient_core::{AbilityId, MethodKey, SignatureHash};

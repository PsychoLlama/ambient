//! Core types for authoring Ambient abilities.
//!
//! This crate provides the fundamental types needed to implement custom abilities:
//!
//! - [`Value`] - Runtime values that abilities work with
//! - [`SuspendedAbility`] - Represents a suspended ability operation
//! - [`VmError`] - Errors that abilities can return
//! - [`HostHandler`] - The function type for ability method implementations
//! - [`RuntimeAbility`] - Trait for defining complete abilities
//!
//! # Example
//!
//! ```ignore
//! use ambient_ability::{RuntimeAbility, HostHandler, Value, SuspendedAbility, VmError};
//! use ambient_core::{AbilityDescriptor, AbilityId, MethodId, TypeFactory};
//!
//! pub struct MyAbility;
//!
//! impl RuntimeAbility for MyAbility {
//!     fn name(&self) -> &'static str { "MyAbility" }
//!     fn ability_id(&self) -> AbilityId { 0x1000 }
//!
//!     fn descriptor<T: Clone + 'static>(&self, factory: &dyn TypeFactory<T>) -> AbilityDescriptor<T> {
//!         // ... define method signatures
//!     }
//!
//!     fn handlers(&self) -> Vec<(MethodId, HostHandler)> {
//!         vec![(0, Box::new(|ability| Ok(Value::Unit)))]
//!     }
//! }
//! ```

#![warn(clippy::print_stdout, clippy::print_stderr)]
#![deny(
    clippy::pedantic,
    clippy::perf,
    clippy::style,
    clippy::complexity,
    clippy::correctness,
    clippy::suspicious,
    clippy::unwrap_used
)]

mod error;
mod format;
mod handler;
mod value;

pub use error::{RuntimeError, StackTraceFrame, VmError};
pub use format::{format_value, format_value_colored, format_value_display};
pub use handler::{HostHandler, RuntimeAbility};
pub use value::{
    CapturedFrame, Closure, Continuation, EnumValue, HandlerValue, MapValue, ModuleExport,
    ModuleExportKind, ModuleMemberRef, ModuleValue, SetValue, SuspendedAbility, Value,
};

// Re-export commonly used types from ambient-core
pub use ambient_core::{AbilityDescriptor, AbilityId, MethodId, TypeFactory};

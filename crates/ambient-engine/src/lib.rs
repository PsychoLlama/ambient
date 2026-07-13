#![warn(clippy::print_stdout, clippy::print_stderr)]
#![deny(
    clippy::pedantic,
    clippy::perf,
    clippy::style,
    clippy::complexity,
    clippy::correctness,
    clippy::suspicious,
    clippy::self_named_module_files
)]
#![cfg_attr(not(test), deny(clippy::expect_used, clippy::unwrap_used))]

pub mod ability_resolver;
pub mod ast;
pub mod build;
pub mod bytecode;
pub mod compilation_cache;
pub mod compiler;
mod const_eval;
pub mod core_library;
pub mod disk_store;
mod dispatch_deps;
pub mod format;
pub mod fqn;
pub mod infer;
pub mod manifest;
pub mod module_env;
pub mod module_path;
pub mod module_registry;
pub mod natives;
pub mod object;
pub mod package;
pub mod protocol;
pub mod resolve;
pub mod store;
pub mod types;
pub mod vm;

// Re-export core types from ambient-ability for backward compatibility
pub use ambient_ability::{SuspendedAbility, Value, VmError};

/// Value types re-exported from ambient-ability.
pub mod value {
    pub use ambient_ability::{
        AbilityMethodRef, CapturedFrame, Closure, Continuation, EnumValue, HandlerValue, MapValue,
        ModuleExport, ModuleExportKind, ModuleMemberRef, ModuleValue, SetValue, SuspendedAbility,
        Value,
    };
}

#[cfg(test)]
pub mod test_utils;

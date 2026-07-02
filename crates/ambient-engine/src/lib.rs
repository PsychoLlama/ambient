#![warn(clippy::print_stdout, clippy::print_stderr)]
#![deny(
    clippy::pedantic,
    clippy::perf,
    clippy::style,
    clippy::complexity,
    clippy::correctness,
    clippy::suspicious,
    clippy::unwrap_used,
    clippy::self_named_module_files
)]
#![cfg_attr(not(test), deny(clippy::expect_used))]

pub mod abilities;
pub mod ability_resolver;
pub mod ast;
pub mod build;
pub mod bytecode;
pub mod compilation_cache;
pub mod compiler;
pub mod core_library;
pub mod disk_store;
pub mod format;
pub mod infer;
pub mod manifest;
pub mod module_path;
pub mod module_registry;
pub mod network_state;
pub mod object;
pub mod package;
pub mod protocol;
pub mod runtime_config;
pub mod store;
pub mod symbol_db;
pub mod types;
pub mod vm;

// Re-export core types from ambient-ability for backward compatibility
pub use ambient_ability::{HostHandler, RuntimeAbility, SuspendedAbility, Value, VmError};

/// Value types re-exported from ambient-ability.
pub mod value {
    pub use ambient_ability::{
        CapturedFrame, Closure, Continuation, EnumValue, HandlerValue, MapValue, ModuleExport,
        ModuleExportKind, ModuleMemberRef, ModuleValue, SetValue, SuspendedAbility, Value,
    };
}

/// Runtime ability trait re-exported from ambient-ability.
pub mod runtime_ability {
    pub use ambient_ability::{HostHandler, RuntimeAbility};
}

#[cfg(test)]
pub mod test_utils;

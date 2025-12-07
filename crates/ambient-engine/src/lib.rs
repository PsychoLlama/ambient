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
pub mod bytecode;
pub mod client;
pub mod compilation_cache;
pub mod compiler;
pub mod core_library;
pub mod format;
pub mod infer;
pub mod manifest;
pub mod module_path;
pub mod module_registry;
pub mod package;
pub mod protocol;
pub mod remote;
pub mod server;
pub mod store;
pub mod types;
pub mod value;
pub mod vm;

#[cfg(test)]
pub mod test_utils;

#![warn(clippy::print_stdout, clippy::print_stderr)]
#![deny(
    clippy::pedantic,
    clippy::perf,
    clippy::style,
    clippy::complexity,
    clippy::correctness,
    clippy::suspicious,
    clippy::unwrap_used,
    clippy::self_named_module_files,
    clippy::shadow_reuse
)]
#![cfg_attr(not(test), deny(clippy::expect_used))]

pub mod abilities;
pub mod ast;
pub mod bytecode;
pub mod client;
pub mod infer;
pub mod protocol;
pub mod remote;
pub mod server;
pub mod store;
pub mod types;
pub mod value;
pub mod vm;

#[cfg(test)]
pub mod test_utils;

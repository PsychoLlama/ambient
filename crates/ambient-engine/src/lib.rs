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
pub mod bytecode;
pub mod content_hash;
pub mod interpreter;
pub mod syntax;
pub mod value;
pub mod vm;

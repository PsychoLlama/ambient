//! Language Server Protocol implementation for the Ambient programming language.
//!
//! This crate provides an LSP server that supports:
//! - Diagnostics (parse and type errors)
//! - Hover information (type annotations)
//! - Go-to-definition navigation
//! - Code completion (keywords, types, functions, variables)
//!
//! # Architecture
//!
//! The LSP server follows the standard request-response model:
//!
//! ```text
//! Client (Editor)          Server (ambient lsp)
//!       |                        |
//!       |-- initialize --------->|
//!       |<-- capabilities -------|
//!       |-- textDocument/open -->|
//!       |<-- diagnostics --------|
//!       |-- textDocument/hover ->|
//!       |<-- type info ----------|
//!       |-- completion --------->|
//!       |<-- completions --------|
//!       |-- shutdown ----------->|
//!       |                        |
//! ```
//!
//! # Usage
//!
//! ```no_run
//! use ambient_lsp::run_server;
//!
//! fn main() -> anyhow::Result<()> {
//!     run_server()
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
    clippy::self_named_module_files
)]
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
#![allow(clippy::module_name_repetitions)]

mod analysis;
mod completions;
mod convert;
mod documents;
mod hover_format;
mod semantic_tokens;
mod server;
mod util;

// Test harness module - exposed publicly so integration tests can use it.
// The module contains testing utilities for the LSP server.
// Testing code has relaxed lint rules.
#[allow(
    dead_code,
    clippy::unwrap_used,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::uninlined_format_args,
    clippy::needless_pass_by_value,
    clippy::ptr_arg,
    clippy::doc_markdown,
    clippy::ref_option,
    clippy::map_unwrap_or
)]
pub mod test_harness;

pub use server::{run_server, run_server_with_connection};

// Re-export key types for library use (REPL, testing)
pub use analysis::{AnalysisResult, analyze, analyze_with_registry, format_type};
pub use completions::{CompletionContext, get_completions};
pub use documents::Document;

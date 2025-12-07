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
    clippy::unwrap_used,
    clippy::self_named_module_files
)]
#![allow(clippy::module_name_repetitions)]

mod analysis;
mod completions;
mod convert;
mod documents;
mod package;
mod server;
mod workspace;

pub use server::run_server;

// Re-export key types for testing
pub use analysis::AnalysisResult;
pub use documents::Document;
pub use workspace::WorkspaceIndex;

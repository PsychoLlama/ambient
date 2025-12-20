//! Test harness for LSP server testing.
//!
//! Provides a fluent builder API for writing LSP tests:
//!
//! ```ignore
//! LspTest::new()
//!     .with_source("fn foo() { let x/*h*/ = 42; x }")
//!     .hover_at("h")
//!     .expect_type("number");
//! ```

mod assertions;
mod builder;
mod client;
mod fixtures;
mod snapshot;

pub use assertions::{CompletionResult, DefinitionResult, HoverResult};
pub use builder::LspTest;
pub use client::TestClient;
pub use fixtures::{parse_markers, Cursor};

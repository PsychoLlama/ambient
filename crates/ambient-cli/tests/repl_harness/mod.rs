//! In-process REPL test harness with a fluent builder API.
//!
//! Drives a real [`ambient_cli::repl::session::ReplSession`] directly rather
//! than spawning `ambient repl` in a PTY. All session and program output
//! flows into one buffer that the builder's assertions poll, so turns are
//! synchronous and deterministic under parallel load.
//!
//! # Example
//!
//! ```ignore
//! mod repl_harness;
//! use repl_harness::ReplTest;
//!
//! #[test]
//! fn test_basic_expression() {
//!     ReplTest::new()
//!         .type_line("1 + 2")
//!         .expect_output("3")
//!         .shutdown();
//! }
//! ```

mod builder;

pub use builder::ReplTest;

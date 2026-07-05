//! REPL test harness with fluent builder API.
//!
//! Provides keystroke-level testing for the REPL by spawning it as a subprocess
//! connected to a pseudo-terminal (PTY). This allows testing the full interactive
//! experience including tab completion, arrow key navigation, and Ctrl sequences.
//!
//! # Example
//!
//! ```ignore
//! mod repl_harness;
//! use repl_harness::{ReplTest, Arrow};
//!
//! #[test]
//! fn test_basic_expression() {
//!     ReplTest::new()
//!         .wait_ready()
//!         .type_line("1 + 2")
//!         .expect_output("3")
//!         .shutdown();
//! }
//!
//! #[test]
//! fn test_tab_completion() {
//!     ReplTest::new()
//!         .wait_ready()
//!         .type_text("Con")
//!         .tab()
//!         .expect_line("Stdio")
//!         .shutdown();
//! }
//!
//! #[test]
//! fn test_history_navigation() {
//!     ReplTest::new()
//!         .wait_ready()
//!         .type_line("1 + 1")
//!         .expect_output("2")
//!         .type_line("2 + 2")
//!         .expect_output("4")
//!         .arrow(Arrow::Up)
//!         .arrow(Arrow::Up)
//!         .enter()
//!         .expect_output("2")
//!         .shutdown();
//! }
//! ```

mod assertions;
mod builder;
mod driver;
mod input;

// Re-export main types for test usage
pub use builder::ReplTest;
pub use input::Arrow;

// These are available for more advanced assertions when needed
#[allow(unused_imports)]
pub use assertions::{LineResult, OutputResult};
#[allow(unused_imports)]
pub use driver::{PtyDriver, PtyError};

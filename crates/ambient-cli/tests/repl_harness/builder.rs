//! Fluent builder for REPL tests.
//!
//! Provides a chainable API for testing the REPL experience, including
//! keystroke simulation, completion testing, and output verification.
//!
//! # Example
//!
//! ```ignore
//! ReplTest::new()
//!     .wait_ready()
//!     .type_line("1 + 2")
//!     .expect_output("3")
//!     .shutdown();
//! ```

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use super::assertions::{LineResult, OutputResult};
use super::driver::PtyDriver;
use super::input::{Arrow, BACKSPACE, ENTER, TAB, ctrl};

/// Default timeout for waiting operations.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Polling interval when waiting for output.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Fluent builder for REPL tests.
///
/// Spawns the REPL as a subprocess connected to a PTY, allowing
/// keystroke-level testing of the interactive experience.
pub struct ReplTest {
    /// PTY driver managing the subprocess.
    driver: PtyDriver,
    /// Default timeout for wait operations.
    timeout: Duration,
    /// Project directory (if set).
    project_dir: Option<PathBuf>,
}

impl ReplTest {
    /// Create a new REPL test with default settings.
    ///
    /// Spawns the REPL subprocess immediately.
    #[must_use]
    pub fn new() -> Self {
        Self::with_options(None, None)
    }

    /// Create a new REPL test with the specified project directory.
    #[must_use]
    pub fn with_project(project_dir: &Path) -> Self {
        Self::with_options(Some(project_dir), None)
    }

    /// Create a new REPL test with custom options.
    fn with_options(project_dir: Option<&Path>, binary_path: Option<&Path>) -> Self {
        let driver =
            PtyDriver::spawn(project_dir, binary_path).expect("failed to spawn REPL subprocess");

        Self {
            driver,
            timeout: DEFAULT_TIMEOUT,
            project_dir: project_dir.map(|p| p.to_path_buf()),
        }
    }

    /// Set the default timeout for wait operations.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    // -------------------------------------------------------------------------
    // Wait / Ready
    // -------------------------------------------------------------------------

    /// Wait for the REPL to be ready (prompt displayed).
    #[must_use]
    pub fn wait_ready(self) -> Self {
        self.wait_for_prompt()
    }

    /// Wait for the prompt to appear.
    #[must_use]
    pub fn wait_for_prompt(self) -> Self {
        self.wait_for(|output| output.contains("> "), "prompt '> '")
    }

    /// Wait for output containing the expected text.
    #[must_use]
    pub fn wait_for_output(self, expected: &str) -> Self {
        self.wait_for(
            |output| output.contains(expected),
            &format!("output containing '{expected}'"),
        )
    }

    /// Wait until a predicate is satisfied.
    fn wait_for(self, pred: impl Fn(&str) -> bool, desc: &str) -> Self {
        let deadline = Instant::now() + self.timeout;

        loop {
            let output = self.driver.output_stripped();
            if pred(&output) {
                return self;
            }

            if Instant::now() > deadline {
                panic!(
                    "Timeout after {:?} waiting for: {}\nActual output:\n{}",
                    self.timeout, desc, output
                );
            }

            thread::sleep(POLL_INTERVAL);
        }
    }

    // -------------------------------------------------------------------------
    // Input: Text
    // -------------------------------------------------------------------------

    /// Type literal text (no special keys).
    ///
    /// Sends each character individually with a small delay to ensure
    /// reliable delivery to the PTY.
    #[must_use]
    pub fn type_text(mut self, text: &str) -> Self {
        for ch in text.bytes() {
            self.driver.write(&[ch]).expect("failed to type text");
            thread::sleep(Duration::from_millis(5));
        }
        self
    }

    /// Type text and press Enter.
    ///
    /// Includes a small delay after typing to ensure the REPL receives all characters
    /// before Enter is pressed.
    #[must_use]
    pub fn type_line(self, text: &str) -> Self {
        let test = self.type_text(text);
        // Small delay to ensure all characters are received before Enter
        thread::sleep(Duration::from_millis(10));
        test.enter()
    }

    // -------------------------------------------------------------------------
    // Input: Special Keys
    // -------------------------------------------------------------------------

    /// Press Enter (submit current line).
    #[must_use]
    pub fn enter(mut self) -> Self {
        self.driver.write(ENTER).expect("failed to press Enter");
        self
    }

    /// Press Tab (for completion).
    #[must_use]
    pub fn tab(mut self) -> Self {
        self.driver.write(TAB).expect("failed to press Tab");
        self
    }

    /// Press an arrow key.
    #[must_use]
    pub fn arrow(mut self, direction: Arrow) -> Self {
        self.driver
            .write(direction.sequence())
            .expect("failed to press arrow key");
        self
    }

    /// Press Ctrl+key combination.
    #[must_use]
    pub fn ctrl(mut self, key: char) -> Self {
        self.driver
            .write(&[ctrl(key)])
            .expect("failed to press Ctrl+key");
        self
    }

    /// Press Backspace.
    #[must_use]
    pub fn backspace(mut self) -> Self {
        self.driver
            .write(BACKSPACE)
            .expect("failed to press Backspace");
        self
    }

    /// Send raw bytes to the PTY.
    #[must_use]
    pub fn raw(mut self, bytes: &[u8]) -> Self {
        self.driver.write(bytes).expect("failed to send raw bytes");
        self
    }

    // -------------------------------------------------------------------------
    // Input: Compound Actions
    // -------------------------------------------------------------------------

    /// Press Tab multiple times (cycle through completions).
    #[must_use]
    pub fn tab_cycle(mut self, n: usize) -> Self {
        for _ in 0..n {
            self = self.tab();
            thread::sleep(Duration::from_millis(50)); // Allow time for completion
        }
        self
    }

    /// Move cursor with arrow keys.
    #[must_use]
    pub fn move_cursor(mut self, direction: Arrow, n: usize) -> Self {
        for _ in 0..n {
            self = self.arrow(direction);
        }
        self
    }

    // -------------------------------------------------------------------------
    // Assertions: Output
    // -------------------------------------------------------------------------

    /// Assert the output contains the expected text.
    ///
    /// Waits until the text appears or times out.
    #[must_use]
    pub fn expect_output(self, expected: &str) -> Self {
        self.wait_for(
            |output| output.contains(expected),
            &format!("output containing '{expected}'"),
        )
    }

    /// Assert the output contains the expected text and return for chained assertions.
    #[must_use]
    pub fn expect_output_result(self, expected: &str) -> OutputResult {
        let test = self.wait_for(
            |output| output.contains(expected),
            &format!("output containing '{expected}'"),
        );
        let output = test.driver.output_stripped();
        OutputResult { test, output }
    }

    /// Assert an error message is shown.
    #[must_use]
    pub fn expect_error(self, msg: &str) -> Self {
        self.wait_for(
            |output| output.contains("error") && output.contains(msg),
            &format!("error containing '{msg}'"),
        )
    }

    /// Assert the prompt is displayed.
    #[must_use]
    pub fn expect_prompt(self) -> Self {
        self.wait_for_prompt()
    }

    /// Get the current output for custom assertions.
    pub fn output(&self) -> String {
        self.driver.output_stripped()
    }

    /// Get the current output including ANSI sequences.
    pub fn output_raw(&self) -> String {
        self.driver.output()
    }

    /// Clear the output buffer.
    ///
    /// Useful for checking only new output after this point.
    #[must_use]
    pub fn clear_output(self) -> Self {
        self.driver.clear_output();
        self
    }

    // -------------------------------------------------------------------------
    // Assertions: Line Content
    // -------------------------------------------------------------------------

    /// Check the current line content (for completion verification).
    ///
    /// This looks for the most recent line after the prompt.
    #[must_use]
    pub fn expect_line(self, expected: &str) -> Self {
        self.wait_for(
            |output| {
                // Find the last line that starts with "> " and check what follows
                output
                    .lines()
                    .rev()
                    .find(|l| l.starts_with("> "))
                    .map(|l| l[2..].contains(expected))
                    .unwrap_or(false)
            },
            &format!("line containing '{expected}'"),
        )
    }

    /// Get the current input line for chained assertions.
    ///
    /// This looks for the LAST occurrence of "> " and returns what follows.
    /// This handles the case where the terminal output has multiple prompts
    /// on the same line (due to cursor control during typing).
    #[must_use]
    pub fn current_line(self) -> LineResult {
        let output = self.driver.output_stripped();
        // Find the last occurrence of "> " and get what follows
        let line = output
            .rfind("> ")
            .map(|pos| output[pos + 2..].to_string())
            .unwrap_or_default();
        LineResult { test: self, line }
    }

    // -------------------------------------------------------------------------
    // Lifecycle
    // -------------------------------------------------------------------------

    /// Gracefully shutdown the REPL.
    pub fn shutdown(mut self) {
        // Try to send :quit command
        let _ = self.driver.write(b":quit\n");
        thread::sleep(Duration::from_millis(100));

        // Force kill if still running
        if self.driver.is_running() {
            self.driver.kill();
        }
    }

    /// Check if the REPL is still running.
    pub fn is_running(&mut self) -> bool {
        self.driver.is_running()
    }
}

impl Default for ReplTest {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ReplTest {
    fn drop(&mut self) {
        // Don't try cleanup if panicking
        if std::thread::panicking() {
            return;
        }

        // Try graceful shutdown
        let _ = self.driver.write(b":quit\n");
    }
}

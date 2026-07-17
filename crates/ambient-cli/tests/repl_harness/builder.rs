//! Fluent builder for in-process REPL tests.
//!
//! Drives a real [`ReplSession`] directly — no subprocess, no PTY. Every
//! line the session emits (control lines, program `Stdio`/`Log` output) and
//! every turn's result (a formatted value, or an `error: …` line) lands in
//! one shared buffer, which the assertions poll. Evaluation is synchronous,
//! so a plain `type_line` needs no sync dance; polling is still needed only
//! because ensured tasks print from their own threads.
//!
//! # Example
//!
//! ```ignore
//! ReplTest::new()
//!     .type_line("1 + 2")
//!     .expect_output("3")
//!     .shutdown();
//! ```

#![allow(dead_code)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ambient_cli::repl::session::{ReplCommand, ReplIo, ReplSession, parse_repl_command};
use ambient_engine::format::format_value;
use ambient_platform::StdioSink;

/// Default deadline for waiting operations. Generous because ensured tasks
/// print asynchronously from their own threads.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Polling interval when waiting for output.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Fluent builder driving an in-process REPL session.
pub struct ReplTest {
    /// The session under test.
    session: ReplSession,
    /// Everything the session and its program print, plus per-turn results.
    buffer: Arc<Mutex<String>>,
    /// Deadline for wait operations.
    timeout: Duration,
}

impl ReplTest {
    /// Create a REPL test with no project (bare core), mirroring
    /// `ambient repl` launched outside a project.
    #[must_use]
    pub fn new() -> Self {
        let dir = std::env::current_dir().expect("current dir");
        Self::at(&dir)
    }

    /// Create a REPL test rooted in `project_dir`, mirroring `ambient repl`
    /// launched in that directory.
    #[must_use]
    pub fn with_project(project_dir: &Path) -> Self {
        Self::at(project_dir)
    }

    /// Build a session whose control and program output both land in one
    /// buffer.
    fn at(project_dir: &Path) -> Self {
        let buffer = Arc::new(Mutex::new(String::new()));

        let control_buf = Arc::clone(&buffer);
        let control: Arc<dyn Fn(&str) + Send + Sync> =
            Arc::new(move |line: &str| append(&control_buf, line));

        let out_buf = Arc::clone(&buffer);
        let err_buf = Arc::clone(&buffer);
        let program = StdioSink::new(
            Arc::new(move |line: &str| append(&out_buf, line)),
            Arc::new(move |line: &str| append(&err_buf, line)),
        );

        let session = ReplSession::new(project_dir, ReplIo::new(control, program))
            .expect("failed to build REPL session");

        Self {
            session,
            buffer,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Set the deadline for wait operations.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    // -------------------------------------------------------------------------
    // Input
    // -------------------------------------------------------------------------

    /// Submit one line: a `:`-command goes through the shared command
    /// handling; anything else is evaluated. Blank lines are ignored (as in
    /// the interactive REPL).
    #[must_use]
    pub fn type_line(mut self, text: &str) -> Self {
        let line = text.trim();
        if line.is_empty() {
            return self;
        }

        if line.starts_with(':') {
            match parse_repl_command(line) {
                // `:quit` is a no-op here; tests wind down via `shutdown`.
                ReplCommand::Quit => {}
                other => {
                    if let Err(e) = self.session.run_command(other) {
                        self.push(&format!("error: {e}"));
                    }
                }
            }
            return self;
        }

        match self.session.eval(line) {
            Ok(Some(value)) => self.push(&format_value(&value)),
            Ok(None) => {}
            Err(e) => self.push(&format!("error: {e}")),
        }
        self
    }

    // -------------------------------------------------------------------------
    // Assertions
    // -------------------------------------------------------------------------

    /// Wait until the buffer contains `expected`, then continue.
    #[must_use]
    pub fn expect_output(self, expected: &str) -> Self {
        self.wait_for(
            |output| output.contains(expected),
            &format!("output containing '{expected}'"),
        )
    }

    /// Wait until the buffer contains `expected` (alias of
    /// [`Self::expect_output`], read as "let the async output catch up").
    #[must_use]
    pub fn wait_for_output(self, expected: &str) -> Self {
        self.expect_output(expected)
    }

    /// Wait until the buffer holds an error line mentioning `msg`.
    #[must_use]
    pub fn expect_error(self, msg: &str) -> Self {
        self.wait_for(
            |output| output.contains("error") && output.contains(msg),
            &format!("error containing '{msg}'"),
        )
    }

    /// The session's completion snapshot, exactly as `cmd_repl` refreshes
    /// it after each handled line — for driving the tab-completion pipeline
    /// over a live session.
    #[must_use]
    pub fn completion_snapshot(&self) -> ambient_cli::repl::session::CompletionSnapshot {
        self.session.completion_snapshot()
    }

    /// The current buffer contents.
    #[must_use]
    pub fn output(&self) -> String {
        self.buffer.lock().expect("buffer lock").clone()
    }

    /// Clear the buffer, so later assertions see only fresh output.
    #[must_use]
    pub fn clear_output(self) -> Self {
        self.buffer.lock().expect("buffer lock").clear();
        self
    }

    /// Poll the buffer until `pred` holds or the deadline passes.
    fn wait_for(self, pred: impl Fn(&str) -> bool, desc: &str) -> Self {
        let deadline = Instant::now() + self.timeout;
        loop {
            let output = self.output();
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

    /// Append a buffer line (from a turn's value/error result).
    fn push(&self, line: &str) {
        append(&self.buffer, line);
    }

    // -------------------------------------------------------------------------
    // Lifecycle
    // -------------------------------------------------------------------------

    /// Wind the session's running tasks down so no ticker keeps printing
    /// into the buffer or holds the test process open.
    pub fn shutdown(self) {
        self.session.shutdown();
    }
}

impl Default for ReplTest {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ReplTest {
    fn drop(&mut self) {
        // Best-effort: stop tasks even if a test panicked before `shutdown`,
        // so a lingering ticker thread can't outlive the buffer.
        self.session.shutdown();
    }
}

/// Append `line` (plus a newline) to the shared buffer.
fn append(buffer: &Arc<Mutex<String>>, line: &str) {
    if let Ok(mut buf) = buffer.lock() {
        buf.push_str(line);
        buf.push('\n');
    }
}

//! PTY driver for spawning and controlling the REPL subprocess.
//!
//! Uses `portable-pty` to create a pseudo-terminal and spawn the REPL,
//! allowing keystroke-level testing of the interactive experience.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

/// Error type for PTY operations.
#[derive(Debug)]
pub struct PtyError(pub String);

impl std::fmt::Display for PtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PtyError {}

/// PTY driver that manages the REPL subprocess.
pub struct PtyDriver {
    /// Handle to the child process.
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Writer to send input to the PTY.
    writer: Box<dyn Write + Send>,
    /// Shared output buffer.
    output: Arc<Mutex<String>>,
    /// Background thread reading from PTY.
    reader_thread: Option<JoinHandle<()>>,
}

impl PtyDriver {
    /// Spawn a new REPL subprocess connected to a PTY.
    ///
    /// # Arguments
    ///
    /// * `project_dir` - Optional project directory for the REPL.
    /// * `binary_path` - Path to the ambient binary (defaults to finding it in PATH or target/).
    pub fn spawn(project_dir: Option<&Path>, binary_path: Option<&Path>) -> Result<Self, PtyError> {
        let pty_system = native_pty_system();

        // Create PTY pair with reasonable terminal size
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError(format!("failed to open PTY: {e}")))?;

        // Build command to spawn
        let binary = binary_path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| find_ambient_binary());

        let mut cmd = CommandBuilder::new(&binary);
        cmd.arg("repl");

        if let Some(dir) = project_dir {
            cmd.cwd(dir);
        }

        // Spawn the REPL process
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError(format!("failed to spawn REPL: {e}")))?;

        // Get reader and writer from master
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PtyError(format!("failed to clone reader: {e}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PtyError(format!("failed to take writer: {e}")))?;

        // Create shared output buffer
        let output = Arc::new(Mutex::new(String::new()));
        let output_clone = Arc::clone(&output);

        // Spawn background thread to read output
        let reader_thread = thread::spawn(move || {
            let mut buf = [0u8; 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]);
                        let mut output = output_clone.lock().unwrap();
                        output.push_str(&text);
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            child,
            writer,
            output,
            reader_thread: Some(reader_thread),
        })
    }

    /// Write bytes to the PTY (simulating keyboard input).
    pub fn write(&mut self, data: &[u8]) -> Result<(), PtyError> {
        self.writer
            .write_all(data)
            .map_err(|e| PtyError(format!("failed to write to PTY: {e}")))?;
        self.writer
            .flush()
            .map_err(|e| PtyError(format!("failed to flush PTY: {e}")))?;
        Ok(())
    }

    /// Get the current output buffer contents.
    pub fn output(&self) -> String {
        self.output.lock().unwrap().clone()
    }

    /// Get the output with ANSI escape sequences stripped.
    pub fn output_stripped(&self) -> String {
        let raw = self.output();
        strip_ansi(&raw)
    }

    /// Clear the output buffer.
    pub fn clear_output(&self) {
        self.output.lock().unwrap().clear();
    }

    /// Check if the child process is still running.
    pub fn is_running(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }

    /// Kill the child process.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for PtyDriver {
    fn drop(&mut self) {
        // Kill the child process
        let _ = self.child.kill();

        // Wait for the reader thread to finish
        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
    }
}

/// Find the ambient binary.
///
/// Uses `CARGO_BIN_EXE_ambient` which cargo sets at compile time for integration tests.
/// This handles both native builds (`target/release/`) and cross-compiled builds
/// (`target/<triple>/release/`) correctly.
fn find_ambient_binary() -> std::path::PathBuf {
    env!("CARGO_BIN_EXE_ambient").into()
}

/// Strip ANSI escape sequences from a string.
fn strip_ansi(s: &str) -> String {
    let bytes = s.as_bytes();
    let stripped = strip_ansi_escapes::strip(bytes);
    String::from_utf8_lossy(&stripped).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi() {
        assert_eq!(strip_ansi("hello"), "hello");
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(strip_ansi("\x1b[1;32mbold green\x1b[0m"), "bold green");
    }
}

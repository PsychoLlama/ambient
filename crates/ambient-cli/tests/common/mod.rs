//! Shared helpers for the CLI integration test binaries.

#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use tempfile::TempDir;

/// Helper to run the ambient CLI command.
pub fn ambient_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ambient"))
}

/// Create a temporary package with the given source as main.ab.
pub fn temp_package(content: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("failed to create temp dir");
    let pkg_dir = dir.path().to_path_buf();

    // Create ambient.toml
    fs::write(
        pkg_dir.join("ambient.toml"),
        r#"[package]
name = "test_pkg"
version = "0.1.0"

[build]
src = "src"
"#,
    )
    .expect("failed to write manifest");

    // Create src/main.ab
    let src_dir = pkg_dir.join("src");
    fs::create_dir_all(&src_dir).expect("failed to create src dir");
    fs::write(src_dir.join("main.ab"), content).expect("failed to write source file");

    (dir, pkg_dir)
}

/// Create a temporary directory with a single source file (for check/compile/ast).
pub fn temp_source(content: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("failed to create temp dir");
    let path = dir.path().join("test.ab");
    fs::write(&path, content).expect("failed to write source file");
    (dir, path)
}

// ─────────────────────────────────────────────────────────────────────────────
// Builder-style Test Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Builder for CLI integration tests.
///
/// Reduces boilerplate for common test patterns: creating temp packages,
/// running CLI commands, and asserting on output.
pub struct CliTest {
    source: String,
    command: String,
    args: Vec<String>,
    _dir: Option<TempDir>,
    path: Option<PathBuf>,
}

#[allow(dead_code)]
impl CliTest {
    /// Create a new CLI test with the given source code.
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            command: "run".into(),
            args: Vec::new(),
            _dir: None,
            path: None,
        }
    }

    /// Use the "compile" command instead of "run".
    pub fn compile(mut self) -> Self {
        self.command = "compile".into();
        self
    }

    /// Use the "check" command instead of "run".
    pub fn check(mut self) -> Self {
        self.command = "check".into();
        self
    }

    /// Use the "ast" command instead of "run".
    pub fn ast(mut self) -> Self {
        self.command = "ast".into();
        self
    }

    /// Add additional arguments to the command.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Execute the command and return the output.
    pub fn execute(&mut self) -> Output {
        let mut cmd = ambient_cmd();

        // For run command, create a full package
        // For other commands, create just a source file
        if self.command == "run" {
            let (dir, pkg_path) = temp_package(&self.source);
            cmd.arg(&self.command).arg(&pkg_path);
            self._dir = Some(dir);
            self.path = Some(pkg_path);
        } else {
            let (dir, file_path) = temp_source(&self.source);
            cmd.arg(&self.command).arg(&file_path);
            self._dir = Some(dir);
            self.path = Some(file_path);
        }

        for arg in &self.args {
            cmd.arg(arg);
        }

        cmd.output().expect("failed to execute command")
    }

    /// Execute and assert success with expected output in stdout.
    pub fn expect_output(mut self, expected: &str) {
        let output = self.execute();
        assert!(
            output.status.success(),
            "{} command failed: {:?}\nstderr: {}",
            self.command,
            output,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(expected),
            "expected '{}' in output: {}",
            expected,
            stdout
        );
    }

    /// Execute and assert success (no specific output check).
    pub fn expect_success(mut self) {
        let output = self.execute();
        assert!(
            output.status.success(),
            "{} command failed: {:?}\nstderr: {}",
            self.command,
            output,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Execute and assert failure.
    pub fn expect_failure(mut self) {
        let output = self.execute();
        assert!(
            !output.status.success(),
            "{} command should have failed",
            self.command
        );
    }

    /// Execute and assert failure with expected text in stderr.
    pub fn expect_error(mut self, expected: &str) {
        let output = self.execute();
        assert!(
            !output.status.success(),
            "{} command should have failed",
            self.command
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(expected),
            "expected '{}' in stderr: {}",
            expected,
            stderr
        );
    }
}

/// Create a temp package with multiple source files: (name, content) pairs,
/// where name is relative to src/ (e.g. "main.ab", "money.ab").
pub fn temp_multi_package(files: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("failed to create temp dir");
    let pkg_dir = dir.path().to_path_buf();

    fs::write(
        pkg_dir.join("ambient.toml"),
        r#"[package]
name = "test_pkg"
version = "0.1.0"

[build]
src = "src"
"#,
    )
    .expect("failed to write manifest");

    let src_dir = pkg_dir.join("src");
    fs::create_dir_all(&src_dir).expect("failed to create src dir");
    for (name, content) in files {
        fs::write(src_dir.join(name), content).expect("failed to write source file");
    }

    (dir, pkg_dir)
}

//! Shared helpers for the CLI integration test binaries.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use ambient_engine::ast::Module;
use ambient_engine::build::{BuildOptions, BuildResult, ParseFailure};
use ambient_engine::disk_store::BuildManifest;
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

// ─────────────────────────────────────────────────────────────────────────────
// Incremental-cache / lazy-build helpers (shared by the engine-level test
// binaries that drive `build_package` directly)
// ─────────────────────────────────────────────────────────────────────────────

/// Parse for the build pipeline, mapping a parse error to the engine's
/// renderable `ParseFailure` (mirrors the CLI's `parse_source`).
pub fn parse_source(source: &str) -> Result<Module, ParseFailure> {
    ambient_parser::parse(source).map_err(|e| ParseFailure {
        message: e.kind.to_string(),
        span: (e.span.start, e.span.end),
        context: e.context,
    })
}

/// Write a package named `pkg_name` from `(src-relative path, source)` pairs
/// into `dir` (an existing, caller-owned directory).
pub fn write_pkg_named(dir: &Path, pkg_name: &str, files: &[(&str, &str)]) {
    fs::write(
        dir.join("ambient.toml"),
        format!("[package]\nname = \"{pkg_name}\"\nversion = \"0.1.0\"\n"),
    )
    .expect("manifest");
    let src = dir.join("src");
    for (rel, body) in files {
        let path = src.join(rel);
        fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        fs::write(path, body).expect("write module");
    }
}

/// Build reading the package's own store (cache Auto), then persist objects +
/// snapshot exactly as `ambient run`/`compile` do — so the next build can hit.
/// Shares the engine's build-and-persist wiring; a persist failure is a hard
/// error here (the tests depend on the snapshot being durable).
pub fn build_and_persist(dir: &Path) -> BuildResult {
    let stubs = ambient_platform::stub_natives();
    let built = ambient_engine::build::build_and_persist(
        dir,
        parse_source,
        BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            ..Default::default()
        },
    )
    .expect("build succeeds");
    built.persisted.expect("persist build");
    built.result
}

/// The canonical cold manifest for a source: build the given files in a fresh
/// package named `pkg_name` (empty store, no snapshot ⇒ everything compiles).
pub fn cold_manifest(pkg_name: &str, files: &[(&str, &str)]) -> BuildManifest {
    let dir = TempDir::new().expect("temp");
    write_pkg_named(dir.path(), pkg_name, files);
    BuildManifest::from_build(&build_and_persist(dir.path()))
}

/// Assert a warm build is byte-identical to a fresh cold build of the same final
/// source (a package named `pkg_name`).
pub fn assert_warm_equals_cold(pkg_name: &str, warm: &BuildResult, files: &[(&str, &str)]) {
    let warm_manifest = BuildManifest::from_build(warm);
    let cold = cold_manifest(pkg_name, files);
    assert_eq!(
        warm_manifest.encode(),
        cold.encode(),
        "warm build must be byte-identical to a fresh cold build"
    );
}

/// `true` when `AMBIENT_CACHE_VERIFY=1` is in this process's environment.
///
/// The in-process cache tests build in-process, so the flag reaches
/// `BuildCache::open` and forces the recompile-and-compare oracle: every module
/// recompiles rather than serving warm. The exact hit/miss-count assertions are
/// then meaningless (a full `AMBIENT_CACHE_VERIFY=1` suite run would fail them
/// for the wrong reason), so each guards on this and skips — the byte-identity
/// (`warm == cold`) checks and the engine's own stale-hit panic remain the
/// oracle. Read the flag exactly the way the cache's `env_flag` does.
pub fn verify_mode() -> bool {
    std::env::var("AMBIENT_CACHE_VERIFY").is_ok_and(|v| v.eq_ignore_ascii_case("1"))
}

/// The package's on-disk incremental store (`<dir>/.ambient/store`).
pub fn store_path(dir: &Path) -> PathBuf {
    dir.join(".ambient").join("store")
}

/// Build-and-persist while capturing each module's `from_cache` flag from the
/// progress callback, so a test can assert *which* modules were served warm
/// (not merely how many). Keyed by the module's dotted path (its basename for a
/// top-level module).
pub fn build_capturing(dir: &Path) -> (BuildResult, std::collections::HashMap<String, bool>) {
    use std::cell::RefCell;
    let seen: RefCell<std::collections::HashMap<String, bool>> = RefCell::new(Default::default());
    let stubs = ambient_platform::stub_natives();
    let result = {
        let cb = |name: &str, _c: usize, _t: usize, from_cache: bool| {
            seen.borrow_mut().insert(name.to_string(), from_cache);
        };
        let built = ambient_engine::build::build_and_persist(
            dir,
            parse_source,
            BuildOptions {
                platform_modules: ambient_platform::platform_modules(),
                natives: Some(&stubs),
                progress: Some(&cb),
                ..Default::default()
            },
        )
        .expect("build succeeds");
        built.persisted.expect("persist build");
        built.result
    };
    (result, seen.into_inner())
}

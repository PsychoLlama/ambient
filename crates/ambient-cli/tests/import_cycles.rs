//! Module import cycles are compile errors.
//!
//! The module dependency graph is a hard DAG (Go's rule): a cycle *between*
//! modules is rejected with a named `import cycle: …` diagnostic, not the
//! arbitrary compile order that used to surface as confusing link failures.
//! Recursion stays *within* a module, so a module referencing its own items
//! is never a cycle. The decision lives once in the engine
//! (`ambient_engine::module_cycles`), shared by the compiling path
//! (`ambient run`) and the analysis path (`ambient check`); these tests
//! drive the real binary so they exercise exactly what a user gets.

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn ambient_bin() -> &'static str {
    env!("CARGO_BIN_EXE_ambient")
}

/// Strip ANSI escape sequences (colors) from output.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for t in chars.by_ref() {
                    if t.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Write a package from `(path, source)` pairs and return its dir.
fn package(files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().expect("create temp dir");
    fs::write(
        dir.path().join("ambient.toml"),
        "[package]\nname = \"cyc\"\nversion = \"0.1.0\"\n",
    )
    .expect("write manifest");
    for (rel, source) in files {
        let path = dir.path().join("src").join(rel);
        fs::create_dir_all(path.parent().expect("has parent")).expect("create dirs");
        fs::write(path, source).expect("write module");
    }
    dir
}

/// Run one subcommand over the package and return combined stdout+stderr
/// plus whether it succeeded.
fn invoke(cmd: &str, dir: &Path) -> (bool, String) {
    let output = Command::new(ambient_bin())
        .arg(cmd)
        .arg(dir)
        .output()
        .expect("failed to spawn ambient");
    let combined = format!(
        "{}{}",
        strip_ansi(&String::from_utf8_lossy(&output.stdout)),
        strip_ansi(&String::from_utf8_lossy(&output.stderr)),
    );
    (output.status.success(), combined)
}

/// A two-module cycle is rejected — with the cycle diagnostic, not a
/// downstream link failure — through both frontends.
#[test]
fn two_module_import_cycle_is_rejected() {
    let dir = package(&[
        ("a.ab", "use pkg::b::bee;\npub fn ay(): Number { bee() }\n"),
        ("b.ab", "use pkg::a::ay;\npub fn bee(): Number { 1 }\n"),
        (
            "main.ab",
            "pub fn run(): Number { pkg::a::ay() + pkg::b::bee() }\n",
        ),
    ]);

    let (ok, run_out) = invoke("run", dir.path());
    assert!(!ok, "run unexpectedly succeeded:\n{run_out}");
    assert!(
        run_out.contains("import cycle: pkg::a -> pkg::b -> pkg::a"),
        "run should name the cycle, not fail with a link error:\n{run_out}"
    );

    let (ok, check_out) = invoke("check", dir.path());
    assert!(!ok, "check unexpectedly succeeded:\n{check_out}");
    assert!(
        check_out.contains("import cycle: pkg::a -> pkg::b -> pkg::a"),
        "check should name the cycle:\n{check_out}"
    );
}

/// A three-module cycle reports the whole path, canonicalized to start at
/// the lexically-least module regardless of which file closes the loop.
#[test]
fn three_module_import_cycle_is_rejected() {
    let dir = package(&[
        ("a.ab", "use pkg::b::bee;\npub fn ay(): Number { bee() }\n"),
        ("b.ab", "use pkg::c::see;\npub fn bee(): Number { see() }\n"),
        ("c.ab", "use pkg::a::ay;\npub fn see(): Number { 1 }\n"),
        ("main.ab", "pub fn run(): Number { pkg::a::ay() }\n"),
    ]);
    let (ok, out) = invoke("check", dir.path());
    assert!(!ok, "check unexpectedly succeeded:\n{out}");
    assert!(
        out.contains("import cycle: pkg::a -> pkg::b -> pkg::c -> pkg::a"),
        "unexpected output:\n{out}"
    );
}

/// A module importing its own items (`use pkg::main::…` from `main`) is a
/// same-module reference, not a cross-module edge — recursion stays within a
/// module — so it is not an import cycle and the package runs.
#[test]
fn self_import_is_not_a_cycle() {
    let dir = package(&[(
        "main.ab",
        "use pkg::main::helper;\n\
         pub fn helper(): Number { 21 }\n\
         pub fn run(): Number { helper() + helper() }\n",
    )]);
    let (ok, out) = invoke("run", dir.path());
    assert!(ok, "run failed:\n{out}");
    assert_eq!(out.trim(), "42");
}

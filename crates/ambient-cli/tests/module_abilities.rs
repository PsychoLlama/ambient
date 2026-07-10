//! End-to-end coverage of user-declared abilities across module boundaries:
//! importing an `ability` from a sibling module works the same whether it is
//! named bare via `use`, reached fully-qualified, re-exported, or reached only
//! through its default implementation — through parse, check, compile, and the
//! VM. The `core::system` platform abilities are just the first consumers of
//! this bridge; these tests pin it for ordinary package abilities too.
//!
//! Split out of `module_system.rs` to keep each test binary under the
//! per-file line budget.

mod common;

use std::path::Path;
use std::process::Command;

use common::temp_multi_package;
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
    temp_multi_package(files).0
}

/// Run the package; expect success and return trimmed stdout.
fn run(dir: &Path) -> String {
    let output = Command::new(ambient_bin())
        .arg("run")
        .arg(dir)
        .output()
        .expect("failed to spawn ambient");
    let stdout = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "run failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    stdout.trim().to_string()
}

/// Check the package; expect failure and return combined output.
fn check_fails(dir: &Path) -> String {
    let output = Command::new(ambient_bin())
        .arg("check")
        .arg(dir)
        .output()
        .expect("failed to spawn ambient");
    let combined = format!(
        "{}{}",
        strip_ansi(&String::from_utf8_lossy(&output.stdout)),
        strip_ansi(&String::from_utf8_lossy(&output.stderr)),
    );
    assert!(
        !output.status.success(),
        "check unexpectedly succeeded:\n{combined}"
    );
    combined
}

#[test]
fn abilities_perform_and_handle_across_modules() {
    let dir = package(&[
        (
            "effects.ab",
            "pub unique(AB000000-0000-0000-0000-000000000012) ability Counter {\n  fn next(): Number { 0 }\n}\n",
        ),
        (
            "main.ab",
            r"
use pkg::effects::Counter;

fn tick(): Number with Counter { Counter::next!() }
fn tock(): Number with pkg::effects::Counter { pkg::effects::Counter::next!() }

pub fn run(): Number {
  with {
    Counter::next() => resume(5)
  } handle tick() + tock()
}
",
        ),
    ]);
    assert_eq!(run(dir.path()), "10");
}

#[test]
fn imported_ability_default_impl_runs_unhandled() {
    // An unhandled perform of an imported user ability runs its default
    // implementation. The entry declares the effect (nothing discharges it),
    // exactly like a `core::system` perform at the top of `run`.
    let dir = package(&[
        (
            "effects.ab",
            "pub unique(AB000000-0000-0000-0000-000000000015) ability Greeter {\n  fn greet(): Number { 42 }\n}\n",
        ),
        (
            "main.ab",
            "use pkg::effects::Greeter;\npub fn run(): Number with Greeter { Greeter::greet!() }\n",
        ),
    ]);
    assert_eq!(run(dir.path()), "42");
}

#[test]
fn imported_ability_dependency_row_runs_across_modules() {
    // An imported ability whose default implementation performs a second
    // imported ability (its `with` dependency) links across the module
    // boundary; both defaults run unhandled from the entry.
    let dir = package(&[
        (
            "effects.ab",
            "pub unique(AB000000-0000-0000-0000-000000000016) ability Base {\n  fn base(): Number { 3 }\n}\n\
             pub unique(AB000000-0000-0000-0000-000000000017) ability Derived with Base {\n  fn derived(): Number { Base::base!() + 10 }\n}\n",
        ),
        (
            "main.ab",
            "use pkg::effects::Derived;\nuse pkg::effects::Base;\n\npub fn run(): Number with Derived, Base { Derived::derived!() }\n",
        ),
    ]);
    assert_eq!(run(dir.path()), "13");
}

#[test]
fn re_exported_ability_imports_through_the_re_exporter() {
    // `pub use` re-exports an ability; importing it from the re-exporting
    // module resolves to the same `AbilityId` and performs correctly.
    let dir = package(&[
        (
            "b.ab",
            "pub unique(AB000000-0000-0000-0000-000000000018) ability Greet {\n  fn hi(): Number { 1 }\n}\n",
        ),
        ("c.ab", "pub use pkg::b::Greet;\n"),
        (
            "main.ab",
            "use pkg::c::Greet;\npub fn run(): Number with Greet { Greet::hi!() }\n",
        ),
    ]);
    assert_eq!(run(dir.path()), "1");
}

#[test]
fn imported_ability_with_nominal_typed_method_signature() {
    // The ability method's signature references a nominal struct from a third
    // module. Canonical-signature rendering keys on the type uuid, so the
    // method resolves the same across the module boundary — handled and via
    // its default implementation.
    let dir = package(&[
        (
            "types.ab",
            "pub unique(D0000000-0000-0000-0000-000000000019) struct Box { n: Number }\n",
        ),
        (
            "effects.ab",
            "use pkg::types::Box;\npub unique(AB000000-0000-0000-0000-00000000001A) ability Store {\n  fn put(x: Number): Box { Box { n: x } }\n}\n",
        ),
        (
            "main.ab",
            r"
use pkg::effects::Store;
use pkg::types::Box;

pub fn run(): Number {
  let handled = with {
    Store::put(x) => resume(Box { n: x * 2 })
  } handle Store::put!(21);
  handled.n
}
",
        ),
    ]);
    assert_eq!(run(dir.path()), "42");
}

#[test]
fn non_public_ability_cannot_be_imported() {
    // Visibility gates the bare-import path: a module-private ability is not
    // importable by name from a sibling module.
    let dir = package(&[
        (
            "effects.ab",
            "unique(AB000000-0000-0000-0000-00000000001B) ability Secret {\n  fn s(): Number { 1 }\n}\n",
        ),
        (
            "main.ab",
            "use pkg::effects::Secret;\npub fn run(): Number with Secret { Secret::s!() }\n",
        ),
    ]);
    let output = check_fails(dir.path());
    assert!(
        output.contains("not public"),
        "expected a visibility error, got:\n{output}"
    );
}

#[test]
fn qualified_handler_arms_match_imported_performs() {
    let dir = package(&[
        (
            "effects.ab",
            "pub unique(AB000000-0000-0000-0000-000000000013) ability Counter {\n  fn next(): Number { 0 }\n}\n",
        ),
        (
            "worker.ab",
            "use pkg::effects::Counter;\npub fn tick(): Number with Counter { Counter::next!() }\n",
        ),
        (
            "main.ab",
            r"
pub fn run(): Number {
  with {
    pkg::effects::Counter::next() => resume(9)
  } handle pkg::worker::tick()
}
",
        ),
    ]);
    assert_eq!(run(dir.path()), "9");
}

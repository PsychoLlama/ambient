//! End-to-end coverage of ability-*method* imports: `use m::Ability::method;`
//! brings one method into scope so perform sites can drop the ability prefix
//! (`seed!(…)` for `Random::seed!(…)`). Calls only — handler arms and effect
//! rows still name the ability (qualified or imported) like before.
//!
//! The dispatch identity is untouched: a bare-method perform resolves to the
//! same `(AbilityId, MethodKey)` as the qualified spelling, so handlers and
//! default implementations behave identically whichever way the call is
//! written. These tests pin that end to end — parse, resolve, check, compile,
//! VM — plus the diagnostics for every way the import can go wrong.

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

/// The donor module every test imports from.
const EFFECTS: &str = "pub unique(AB000000-0000-0000-0000-000000000021) ability Counter {\n  fn next(): Number { 7 }\n  fn reset(): Number { 0 }\n}\n";

#[test]
fn bare_method_perform_dispatches_like_the_qualified_spelling() {
    // One handler covers both spellings: the bare perform derives the same
    // (AbilityId, MethodKey), so the arm intercepts it identically.
    let dir = package(&[
        ("effects.ab", EFFECTS),
        (
            "main.ab",
            r"
use pkg::effects::{Counter, Counter::next};

fn bare(): Number with Counter { next!() }
fn qualified(): Number with Counter { Counter::next!() }

pub fn run(): Number {
  with {
    Counter::next() => resume(5)
  } handle bare() + qualified()
}
",
        ),
    ]);
    assert_eq!(run(dir.path()), "10");
}

#[test]
fn bare_method_perform_runs_the_default_impl_unhandled() {
    let dir = package(&[
        ("effects.ab", EFFECTS),
        (
            "main.ab",
            "use pkg::effects::{Counter, Counter::next};\npub fn run(): Number with Counter { next!() }\n",
        ),
    ]);
    assert_eq!(run(dir.path()), "7");
}

#[test]
fn method_import_alias_binds_the_local_name() {
    // `as` renames the local binding; resolution rewrites it back to the
    // declared method, so dispatch is unchanged.
    let dir = package(&[
        ("effects.ab", EFFECTS),
        (
            "main.ab",
            r"
use pkg::effects::Counter;
use pkg::effects::Counter::next as tick;

pub fn run(): Number with Counter { tick!() }
",
        ),
    ]);
    assert_eq!(run(dir.path()), "7");
}

#[test]
fn platform_ability_method_imports_bare() {
    // The same machinery covers `core::system`: importing `Stdio::out`
    // makes the bare `out!` perform reach the platform native.
    let dir = package(&[(
        "main.ab",
        r#"
use core::system::Stdio::out;

pub fn run(): () with core::system::Stdio {
  out!("bare perform")
}
"#,
    )]);
    assert_eq!(run(dir.path()), "bare perform");
}

#[test]
fn block_scoped_method_import_binds_to_end_of_block() {
    let dir = package(&[
        ("effects.ab", EFFECTS),
        (
            "main.ab",
            r"
use pkg::effects::Counter;

pub fn run(): Number with Counter {
  use pkg::effects::Counter::next;
  next!()
}
",
        ),
    ]);
    assert_eq!(run(dir.path()), "7");
}

#[test]
fn method_import_through_a_re_export_resolves_to_the_defining_ability() {
    let dir = package(&[
        ("effects.ab", EFFECTS),
        (
            "facade.ab",
            "pub use pkg::effects::Counter;\npub use pkg::effects::Counter::next;\n",
        ),
        (
            "main.ab",
            r"
use pkg::facade::{Counter, next};

pub fn run(): Number {
  with { Counter::next() => resume(3) } handle next!()
}
",
        ),
    ]);
    assert_eq!(run(dir.path()), "3");
}

#[test]
fn same_module_method_import_via_self_works() {
    // `use self::…` imports a sibling declaration's method; the resolved
    // ability lands in the compiler's *local* table rather than the
    // foreign channel — a distinct path worth pinning.
    let dir = package(&[(
        "main.ab",
        r"
pub unique(AB000000-0000-0000-0000-000000000023) ability Local {
  fn hello(): Number { 11 }
}

use self::Local::hello;

pub fn run(): Number with Local { hello!() }
",
    )]);
    assert_eq!(run(dir.path()), "11");
}

#[test]
fn importing_a_missing_method_reports_the_method_error() {
    let dir = package(&[
        ("effects.ab", EFFECTS),
        (
            "main.ab",
            "use pkg::effects::Counter::nope;\npub fn run(): Number { 1 }\n",
        ),
    ]);
    let out = check_fails(dir.path());
    assert!(
        out.contains("ability `Counter` has no method `nope`"),
        "missing method should name the ability and method:\n{out}"
    );
}

#[test]
fn private_ability_methods_cannot_be_imported() {
    let dir = package(&[
        (
            "effects.ab",
            "unique(AB000000-0000-0000-0000-000000000022) ability Secret {\n  fn peek(): Number { 1 }\n}\n",
        ),
        (
            "main.ab",
            "use pkg::effects::Secret::peek;\npub fn run(): Number { 1 }\n",
        ),
    ]);
    let out = check_fails(dir.path());
    assert!(
        out.contains("not public"),
        "private ability should gate its methods:\n{out}"
    );
}

#[test]
fn prelude_throw_performs_bare_and_catches_like_the_qualified_spelling() {
    // `Exception::throw` rides the prelude as a method re-export, so
    // `throw!(…)` needs no import — and one handler arm (still spelled
    // qualified) catches both spellings.
    let dir = package(&[(
        "main.ab",
        r#"
fn bare(): Number with Exception { throw!("bare") }
fn qualified(): Number with Exception { Exception::throw!("qualified") }

pub fn run(): Number {
  let a = with { Exception::throw(e) => 1 } handle bare();
  let b = with { Exception::throw(e) => 2 } handle qualified();
  a + b
}
"#,
    )]);
    assert_eq!(run(dir.path()), "3");
}

#[test]
fn explicit_method_import_shadows_the_prelude_throw() {
    // An explicit `use` of another ability's `throw` wins over the
    // prelude's `Exception::throw` under the bare name; the prelude one
    // stays reachable qualified.
    let dir = package(&[
        (
            "effects.ab",
            "pub unique(AB000000-0000-0000-0000-000000000024) ability Soft {\n  fn throw(msg: String): Number { 5 }\n}\n",
        ),
        (
            "main.ab",
            r#"
use pkg::effects::{Soft, Soft::throw};

pub fn run(): Number with Soft { throw!("soft") }
"#,
        ),
    ]);
    assert_eq!(run(dir.path()), "5");
}

#[test]
fn bare_perform_without_an_import_suggests_the_use_path() {
    let dir = package(&[
        ("effects.ab", EFFECTS),
        (
            "main.ab",
            "use pkg::effects::Counter;\npub fn run(): Number with Counter { next!() }\n",
        ),
    ]);
    let out = check_fails(dir.path());
    assert!(
        out.contains("no ability method `next` in scope"),
        "bare perform without import should be diagnosed:\n{out}"
    );
    assert!(
        out.contains("use pkg::effects::Counter::next;"),
        "diagnostic should suggest the importable path:\n{out}"
    );
}

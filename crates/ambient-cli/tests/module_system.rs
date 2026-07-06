//! End-to-end coverage of the module system: every spelling of a
//! reference — bare import, module alias, inline rooted path — resolves
//! to the same canonical identity, and the rule "anything reachable
//! fully-qualified works through `use`, and vice versa" holds through
//! parse, check, compile, and the VM.
//!
//! Each test builds a real package on disk and runs it through the
//! `ambient` binary, so it exercises exactly what a user gets.

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
        "[package]\nname = \"modsys\"\nversion = \"0.1.0\"\n",
    )
    .expect("write manifest");
    for (rel, source) in files {
        let path = dir.path().join("src").join(rel);
        fs::create_dir_all(path.parent().expect("has parent")).expect("create dirs");
        fs::write(path, source).expect("write module");
    }
    dir
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

/// Check the package; expect success.
fn check_passes(dir: &Path) {
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
    assert!(output.status.success(), "check failed:\n{combined}");
}

// ─────────────────────────────────────────────────────────────────────────
// Deep module trees and directory namespaces
// ─────────────────────────────────────────────────────────────────────────

const LEAF: &str = "pub fn leaf_fn(): Number { 7 }\n";

#[test]
fn every_spelling_reaches_a_deep_module() {
    // The same function referenced five ways: item import, whole-module
    // import of the file, whole-module import of a directory namespace,
    // chained module aliases, and an inline rooted path.
    let dir = package(&[
        ("deep/nested/leaf.ab", LEAF),
        (
            "main.ab",
            r"
use pkg::deep::nested::leaf::leaf_fn;
use pkg::deep::nested::leaf;
use pkg::deep::nested;
use pkg::deep;
use deep::nested as nested2;

pub fn run(): Number {
  leaf_fn()
    + leaf::leaf_fn()
    + nested::leaf::leaf_fn()
    + deep::nested::leaf::leaf_fn()
    + nested2::leaf::leaf_fn()
    + pkg::deep::nested::leaf::leaf_fn()
}
",
        ),
    ]);
    assert_eq!(run(dir.path()), "42");
}

#[test]
fn self_and_super_are_directory_relative() {
    let dir = package(&[
        ("a/b/mid.ab", "pub fn mid(): Number { 10 }\n"),
        (
            "a/b/user.ab",
            "use self::mid::mid;\npub fn both(): Number { mid() + super::top::top() }\n",
        ),
        ("a/top.ab", "pub fn top(): Number { 3 }\n"),
        (
            "main.ab",
            "pub fn run(): Number { pkg::a::b::user::both() }\n",
        ),
    ]);
    assert_eq!(run(dir.path()), "13");
}

// ─────────────────────────────────────────────────────────────────────────
// Use-tree syntax
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn use_trees_flatten_to_plain_imports() {
    let dir = package(&[
        (
            "util.ab",
            "pub fn double(x: Number): Number { x * 2 }\npub const ANSWER: Number = 42;\n",
        ),
        ("deep/nested/leaf.ab", LEAF),
        (
            "main.ab",
            r"
use {pkg::util::double, pkg::deep::{nested::leaf::leaf_fn as leaf7}};
use core::Number::sqrt as root;

pub fn run(): Number {
  double(leaf7()) + root(4)
}
",
        ),
    ]);
    assert_eq!(run(dir.path()), "16");
}

#[test]
fn use_works_in_blocks_and_scopes_to_them() {
    let dir = package(&[
        ("util.ab", "pub const ANSWER: Number = 42;\n"),
        (
            "main.ab",
            r"
pub fn run(): Number {
  let a = {
    use pkg::util::ANSWER;
    ANSWER
  };
  a + pkg::util::ANSWER
}
",
        ),
    ]);
    assert_eq!(run(dir.path()), "84");
}

#[test]
fn block_import_does_not_leak_out_of_its_block() {
    let dir = package(&[
        ("util.ab", "pub const ANSWER: Number = 42;\n"),
        (
            "main.ab",
            r"
pub fn run(): Number {
  let a = { use pkg::util::ANSWER; ANSWER };
  ANSWER
}
",
        ),
    ]);
    let out = check_fails(dir.path());
    assert!(out.contains("ANSWER"), "unexpected output:\n{out}");
}

// ─────────────────────────────────────────────────────────────────────────
// Intrinsics behave like ordinary core items
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn intrinsics_import_and_alias_like_functions() {
    let dir = package(&[(
        "main.ab",
        r"
use core::Number::sqrt;
use core::Number;
use core::List;

pub fn run(): Number {
  sqrt(16) + Number::sqrt(16) + core::Number::sqrt(16) + List::length(List::range(1, 4))
}
",
    )]);
    assert_eq!(run(dir.path()), "15");
}

// ─────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn constants_resolve_through_every_spelling() {
    let dir = package(&[
        ("util.ab", "pub const ANSWER: Number = 42;\n"),
        (
            "main.ab",
            r"
use pkg::util::ANSWER;
use pkg::util;

pub fn run(): Number { ANSWER + util::ANSWER + pkg::util::ANSWER }
",
        ),
    ]);
    assert_eq!(run(dir.path()), "126");
}

// ─────────────────────────────────────────────────────────────────────────
// Abilities across modules
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn abilities_perform_and_handle_across_modules() {
    let dir = package(&[
        (
            "effects.ab",
            "pub ability Counter {\n  fn next(): Number;\n}\n",
        ),
        (
            "main.ab",
            r"
use pkg::effects::Counter;

fn tick(): Number with Counter { Counter::next!() }
fn tock(): Number with pkg::effects::Counter { pkg::effects::Counter::next!() }

pub fn run(): Number {
  handle tick() + tock() {
    Counter::next() => resume(5)
  }
}
",
        ),
    ]);
    assert_eq!(run(dir.path()), "10");
}

#[test]
fn qualified_handler_arms_match_imported_performs() {
    let dir = package(&[
        (
            "effects.ab",
            "pub ability Counter {\n  fn next(): Number;\n}\n",
        ),
        (
            "worker.ab",
            "use pkg::effects::Counter;\npub fn tick(): Number with Counter { Counter::next!() }\n",
        ),
        (
            "main.ab",
            r"
pub fn run(): Number {
  handle pkg::worker::tick() {
    pkg::effects::Counter::next() => resume(9)
  }
}
",
        ),
    ]);
    assert_eq!(run(dir.path()), "9");
}

// ─────────────────────────────────────────────────────────────────────────
// Types across modules
// ─────────────────────────────────────────────────────────────────────────

const SHAPES: &str = r"
pub unique(D098767B-4093-4D5C-BA37-AD92AA7B5D01) struct Money { cents: Number }
pub unique(D098767B-4093-4D5C-BA37-AD92AA7B5D02) enum Shape { Circle(Number), Dot }
";

#[test]
fn qualified_types_work_in_annotations_and_constructors() {
    let dir = package(&[
        ("shapes.ab", SHAPES),
        (
            "main.ab",
            r"
pub fn run(): Number {
  let m: pkg::shapes::Money = pkg::shapes::Money { cents: 7 };
  m.cents
}
",
        ),
    ]);
    assert_eq!(run(dir.path()), "7");
}

#[test]
fn qualified_enum_type_unifies_with_imported_constructors() {
    let dir = package(&[
        ("shapes.ab", SHAPES),
        (
            "main.ab",
            r"
use pkg::shapes::Shape;

pub fn area(s: pkg::shapes::Shape): Number {
  match s { Circle(r) => r * r, Dot => 0 }
}

pub fn run(): Number { area(Circle(3)) }
",
        ),
    ]);
    assert_eq!(run(dir.path()), "9");
}

#[test]
fn unit_struct_constructs_across_modules_both_spellings() {
    // A unit struct is a value reachable by bare name. It must construct from
    // another module both ways the access invariant requires: through
    // `use m::{Origin}` and fully-qualified `m::Origin`. Both spellings
    // canonicalize to `<module>::Origin`, so both flow into `accepts`.
    let dir = package(&[
        (
            "markers.ab",
            r"
pub unique(D098767B-4093-4D5C-BA37-AD92AA7B5D03) struct Origin;

pub fn accepts(_o: Origin): Number { 5 }
",
        ),
        (
            "main.ab",
            r"
use pkg::markers::{Origin, accepts};

pub fn run(): Number {
  let imported = Origin;
  let qualified = pkg::markers::Origin;
  accepts(imported) + accepts(qualified)
}
",
        ),
    ]);
    assert_eq!(run(dir.path()), "10");
}

// ─────────────────────────────────────────────────────────────────────────
// Re-exports
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn re_exports_resolve_to_their_origin() {
    let dir = package(&[
        ("origin.ab", "pub fn helper(): Number { 11 }\n"),
        (
            "facade.ab",
            "pub use pkg::origin::helper;\npub use pkg::origin::helper as aka;\npub use pkg::origin;\n",
        ),
        (
            "main.ab",
            r"
use pkg::facade::helper;
use pkg::facade::aka;

pub fn run(): Number {
  helper() + aka() + pkg::facade::helper() + pkg::facade::origin::helper()
}
",
        ),
    ]);
    assert_eq!(run(dir.path()), "44");
}

// ─────────────────────────────────────────────────────────────────────────
// Failure modes stay failures
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn private_items_stay_private_through_paths() {
    let dir = package(&[
        ("util.ab", "fn secret(): Number { 1 }\n"),
        ("main.ab", "pub fn run(): Number { pkg::util::secret() }\n"),
    ]);
    let out = check_fails(dir.path());
    assert!(out.contains("secret"), "unexpected output:\n{out}");
}

#[test]
fn missing_import_is_an_error_at_the_use_item() {
    let dir = package(&[(
        "main.ab",
        "use pkg::nonexistent::thing;\npub fn run(): Number { 1 }\n",
    )]);
    let out = check_fails(dir.path());
    assert!(out.contains("import failed"), "unexpected output:\n{out}");
}

#[test]
fn unresolved_alias_head_is_an_error() {
    let dir = package(&[(
        "main.ab",
        "use nowhere::thing;\npub fn run(): Number { 1 }\n",
    )]);
    let out = check_fails(dir.path());
    assert!(out.contains("nowhere"), "unexpected output:\n{out}");
}

#[test]
fn reserved_root_module_names_are_rejected() {
    let dir = package(&[
        ("core.ab", "pub fn f(): Number { 1 }\n"),
        ("main.ab", "pub fn run(): Number { 1 }\n"),
    ]);
    let out = check_fails(dir.path());
    assert!(out.contains("reserved"), "unexpected output:\n{out}");
}

#[test]
fn check_and_run_agree_on_the_full_matrix() {
    // The parity meta-check: a package touching every feature checks
    // clean and runs.
    let dir = package(&[
        ("shapes.ab", SHAPES),
        ("util.ab", "pub fn double(x: Number): Number { x * 2 }\n"),
        ("deep/nested/leaf.ab", LEAF),
        (
            "main.ab",
            r"
use {pkg::util::double, pkg::shapes::Shape};
use pkg::deep::nested;

pub fn run(): Number {
  let s: Shape = Circle(nested::leaf::leaf_fn());
  match s { Circle(r) => double(r), Dot => 0 }
}
",
        ),
    ]);
    check_passes(dir.path());
    assert_eq!(run(dir.path()), "14");
}

// ─────────────────────────────────────────────────────────────────────────
// `extern` structs: engine-provided, unconstructable across module boundaries
// ─────────────────────────────────────────────────────────────────────────

/// An `extern` struct exported by one module cannot be constructed by another.
/// The ban keys off the type's canonical nominal identity (its UUID), not the
/// local spelling — so importing it and writing `T { .. }` is rejected, proving
/// enforcement is not a local-only, definition-site concern.
#[test]
fn extern_struct_cannot_be_constructed_across_modules() {
    let dir = package(&[
        (
            "handles.ab",
            "pub extern unique(A1B2C3D4-0000-0000-0000-0000000000AB) struct Handle { id: Number }\n",
        ),
        (
            "main.ab",
            r"
use pkg::handles::Handle;

pub fn run(): Number {
  let h: Handle = Handle { id: 1 };
  h.id
}
",
        ),
    ]);
    let output = check_fails(dir.path());
    assert!(
        output.contains("provided by the engine"),
        "expected an extern-construction error, got:\n{output}"
    );
}

/// The same extern struct may be named in a signature and have its fields read
/// across modules — only construction is banned. A function taking a `Handle`
/// and reading `h.id` checks clean.
#[test]
fn extern_struct_can_be_named_and_read_across_modules() {
    let dir = package(&[
        (
            "handles.ab",
            "pub extern unique(A1B2C3D4-0000-0000-0000-0000000000AC) struct Handle { id: Number }\n",
        ),
        (
            "main.ab",
            r"
use pkg::handles::Handle;

pub fn describe(h: Handle): Number { h.id }
",
        ),
    ]);
    check_passes(dir.path());
}

/// The built-in primitives (`String`/`Number`/`Bool`/`Bytes`) are `extern`
/// declarations in `core`, so their bare unit form is not a value: `let x:
/// String = String;` fails. This is the footgun that made a *constructible*
/// primitive declaration inexpressible before `extern` landed.
#[test]
fn primitive_bare_name_is_not_a_value() {
    let dir = package(&[(
        "main.ab",
        "pub fn run(): String {\n  let x: String = String;\n  x\n}\n",
    )]);
    let output = check_fails(dir.path());
    assert!(
        output.contains("undefined variable"),
        "expected `String` to have no value binding, got:\n{output}"
    );
}

/// A primitive cannot be constructed with the record form either — the same
/// `extern` construction ban that guards user `extern` structs.
#[test]
fn primitive_cannot_be_constructed() {
    let dir = package(&[(
        "main.ab",
        "pub fn run(): Number {\n  let x = String { value: 1 };\n  2\n}\n",
    )]);
    let output = check_fails(dir.path());
    assert!(
        output.contains("provided by the engine"),
        "expected an extern-construction error, got:\n{output}"
    );
}

/// Bare `String`/`Number`/`Bool` resolve in every module without a `use` — they
/// are prelude, like `Option`/`Result`. A module that never imports `core`
/// still type-checks annotations and literals against them.
#[test]
fn primitives_resolve_without_import() {
    let dir = package(&[(
        "main.ab",
        r#"
pub fn run(n: Number, b: Bool): String {
  if b { "yes" } else { "no" }
}
"#,
    )]);
    check_passes(dir.path());
}

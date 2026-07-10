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
            r#"
use {pkg::util::double, pkg::deep::{nested::leaf::leaf_fn as leaf7}};
use core::convert::parse_number as root;

pub fn run(): Number {
  double(leaf7()) + root("2").unwrap_or(0)
}
"#,
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
        r#"
use core::convert::parse_number;
use core::convert;

pub fn run(): Number {
  parse_number("4").unwrap_or(0)
    + convert::parse_number("4").unwrap_or(0)
    + core::convert::parse_number("4").unwrap_or(0)
}
"#,
    )]);
    assert_eq!(run(dir.path()), "12");
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
fn fully_qualified_enum_variant_constructs_across_modules() {
    // The explicit-enum spelling `pkg::shapes::Shape::Circle(3)` — where the
    // last path segment names an enum, not a module — constructs the same
    // value the bare/imported `Circle(3)` does, end-to-end. Both the payload
    // variant (`Circle`, via a call) and the unit variant (`Dot`, as a value)
    // resolve through the qualified channel.
    let dir = package(&[
        ("shapes.ab", SHAPES),
        (
            "main.ab",
            r"
use pkg::shapes::Shape;

pub fn area(s: Shape): Number {
  match s { Circle(r) => r * r, Dot => 0 }
}

pub fn run(): Number {
  area(pkg::shapes::Shape::Circle(3)) + area(pkg::shapes::Shape::Dot)
}
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
// Same-named enums in different modules stay distinct types
// ─────────────────────────────────────────────────────────────────────────

/// `other` and `main` each declare their own `enum Color` with distinct
/// uuids.
const OTHER_COLOR: &str =
    "pub unique(D098767B-4093-4D5C-BA37-AD92AA7B5DA1) enum Color { Foreign }\n";

/// A function annotated with the *foreign* `pkg::other::Color` must keep that
/// identity through hole resolution. Previously `resolve_holes` re-keyed any
/// `Named("Color", ..)` to whatever enum the local registry held under that
/// bare name — even when the resolve pass had already stamped the foreign
/// uuid — so a same-named local `Color` silently unified with the foreign
/// annotation. Passing a locally constructed value must be a type mismatch.
#[test]
fn foreign_enum_annotation_is_not_rekeyed_to_a_same_named_local() {
    let dir = package(&[
        ("other.ab", OTHER_COLOR),
        (
            "main.ab",
            r"
pub unique(D098767B-4093-4D5C-BA37-AD92AA7B5DA2) enum Color { Local }

pub fn wants(c: pkg::other::Color): Number { 0 }

pub fn run(): Number { wants(Local) }
",
        ),
    ]);
    let out = check_fails(dir.path());
    assert!(
        out.to_lowercase().contains("mismatch") || out.contains("Color"),
        "expected a type mismatch between the two `Color`s, got:\n{out}"
    );
}

/// The other side of the same coin: the foreign annotation still accepts a
/// value constructed through the foreign enum. The fix preserves the
/// resolve-pass uuid rather than dropping the annotation's identity, so this
/// keeps checking.
#[test]
fn foreign_enum_annotation_accepts_a_foreign_constructed_value() {
    let dir = package(&[
        ("other.ab", OTHER_COLOR),
        (
            "main.ab",
            r"
pub unique(D098767B-4093-4D5C-BA37-AD92AA7B5DA2) enum Color { Local }

pub fn wants(c: pkg::other::Color): Number { 0 }

pub fn run(): Number { wants(pkg::other::Color::Foreign) }
",
        ),
    ]);
    check_passes(dir.path());
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
fn cross_module_ability_dependency_cycle_is_reported() {
    // `a::Ay` depends on `b::Bee` and vice versa. Without dedicated
    // detection this only ever surfaced as a module-cycle link failure that
    // never mentioned the abilities; cycle detection reports it as an
    // ability dependency cycle, at each participating declaration.
    let dir = package(&[
        (
            "a.ab",
            r"
use pkg::b::Bee;

pub unique(AB000000-0000-0000-0000-000000000020) ability Ay with Bee {
    fn ay(): () { () }
}
",
        ),
        (
            "b.ab",
            r"
use pkg::a::Ay;

pub unique(AB000000-0000-0000-0000-000000000021) ability Bee with Ay {
    fn bee(): () { () }
}
",
        ),
        ("main.ab", "pub fn run(): Number { 7 }\n"),
    ]);
    let out = check_fails(dir.path());
    assert!(
        out.contains("ability dependency cycle"),
        "unexpected output:\n{out}"
    );
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

/// The built-in primitives (`String`/`Number`/`Bool`/`Binary`) are `extern`
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

/// Bare `Some`/`None`/`Ok`/`Err` and `Option<T>`/`Result<T, E>` resolve in
/// every module without a `use` — they are the prelude, re-exported by
/// `core::prelude` and injected at lowest precedence. Construct, pattern
/// match, and run, all with no imports.
#[test]
fn prelude_enums_resolve_without_import() {
    let dir = package(&[(
        "main.ab",
        r"
fn unwrap(o: Option<Number>): Number {
  match o { Some(n) => n, None => 0 }
}

fn collapse(r: Result<Number, Number>): Number {
  match r { Ok(n) => n, Err(e) => e }
}

pub fn run(): Number {
  unwrap(Some(4)) + unwrap(None) + collapse(Ok(2)) + collapse(Err(3))
}
",
    )]);
    assert_eq!(run(dir.path()), "9");
}

/// A local declaration shadows the prelude: a user `enum E { Some, None }`
/// binds `Some`/`None` to *its* variants, not `core::option`'s. The match
/// arms and constructor resolve to the local enum end-to-end.
#[test]
fn user_declaration_shadows_prelude() {
    let dir = package(&[(
        "main.ab",
        r"
pub unique(D098767B-4093-4D5C-BA37-AD92AA7B5D09) enum E { Some(Number), None }

fn tag(e: E): Number {
  match e { Some(n) => n, None => 0 }
}

pub fn run(): Number {
  tag(Some(7)) + tag(None)
}
",
    )]);
    assert_eq!(run(dir.path()), "7");
}

/// An explicit `use core::option::Option;` names the very enum the prelude
/// already injects. The two must coexist rather than collide: the import
/// takes precedence at the same origin, and `Option`/`Some`/`None` still
/// resolve and run. (Variants can't be imported piecemeal — the enum import
/// carries them — so this is the meaningful "explicit use + prelude" case.)
#[test]
fn explicit_enum_import_coexists_with_prelude() {
    let dir = package(&[(
        "main.ab",
        r"
use core::option::Option;

fn unwrap(o: Option<Number>): Number {
  match o { Some(n) => n, None => 0 }
}

pub fn run(): Number { unwrap(Some(5)) + unwrap(None) }
",
    )]);
    assert_eq!(run(dir.path()), "5");
}

// ─────────────────────────────────────────────────────────────────────────
// The `core::` hierarchy is file-defined and arbitrary-depth
// ─────────────────────────────────────────────────────────────────────────

/// A deep core path walks through its namespace parents: `core::collections`
/// and `core::collections::list` are real registered modules, so both the
/// `use` alias spelling and the inline fully-qualified spelling reach the
/// same `List` type — the access-rule invariant, now through two namespace
/// levels. (The module's helper functions are private; its public export is
/// the type.)
#[test]
fn core_collections_list_reaches_the_type_both_ways() {
    let dir = package(&[(
        "main.ab",
        r#"
use core::collections::list;

pub fn run(): Number {
  // Alias spelling and inline rooted spelling name the same `List` type.
  let viaAlias: list::List<Number> = List::range(0, 3);
  let viaPath: core::collections::list::List<Number> = List::range(0, 3);
  viaAlias.length() + viaPath.length()
}
"#,
    )]);
    assert_eq!(run(dir.path()), "6");
}

/// The low-level `sqrt` extern is module-private: the inherent method
/// `(16).sqrt()` is the whole public surface, and the qualified extern path
/// no longer resolves.
#[test]
fn core_primitives_number_sqrt_is_method_only() {
    let dir = package(&[(
        "main.ab",
        r#"
pub fn run(): Number {
  (16).sqrt() + (16).sqrt()
}
"#,
    )]);
    assert_eq!(run(dir.path()), "8");

    let dir = package(&[(
        "main.ab",
        r#"
pub fn run(): Number {
  core::primitives::number::sqrt(16)
}
"#,
    )]);
    let out = check_fails(dir.path());
    assert!(out.contains("sqrt"), "unexpected output:\n{out}");
}

// ─────────────────────────────────────────────────────────────────────────
// Shadowing: a local binding must win over a same-named top-level item.
//
// Since same-module references now resolve to the module's `Fqn`, a local
// that shadows a top-level function or const must still load the local —
// the resolve pass's lexical `is_local` tracking has to be complete across
// every binding form, or a shadowing local misresolves to a module `Fqn`
// and emits a function ref / inlined const instead of a local load.

#[test]
fn locals_shadow_top_level_functions_in_every_binding_form() {
    let dir = package(&[(
        "main.ab",
        r"
fn helper(): Number { 100 }

// A parameter named `helper` shadows the top-level function.
fn shadow_param(helper: Number): Number { helper }

// A `let` binding shadows it.
fn shadow_let(): Number { let helper = 7; helper }

// A lambda parameter shadows it.
fn shadow_lambda(): Number { let f = (helper) => helper; f(7) }

// A match binding (lowercase = irrefutable binding) shadows it.
fn shadow_match(): Number { match 7 { helper => helper } }

pub fn run(): Number {
  shadow_param(1) + shadow_let() + shadow_lambda() + shadow_match()
}
",
    )]);
    // 1 + 7 + 7 + 7 = 22. If any local misresolved to the top-level
    // `helper`, the body would be a function ref (a type error) or 100.
    assert_eq!(run(dir.path()), "22");
}

#[test]
fn a_parameter_shadows_a_top_level_const() {
    // A const inlines its literal at every reference; a parameter of the
    // same name must load the argument, not the inlined const value.
    let dir = package(&[(
        "main.ab",
        r"
const LIMIT: Number = 100;

fn echo(LIMIT: Number): Number { LIMIT }

pub fn run(): Number { echo(7) }
",
    )]);
    assert_eq!(run(dir.path()), "7");
}

// ─────────────────────────────────────────────────────────────────────────
// Container types resolve through scope, not a global name table
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn containers_resolve_bare_qualified_and_imported() {
    // `List`/`Map`/`Set` are ordinary prelude types: the bare spelling comes
    // from `core::prelude`, the fully-qualified spelling needs no import,
    // and an explicit `use` binds the same identity. All three must denote
    // one type — a list built under one spelling flows through the others.
    let dir = package(&[
        (
            "bare.ab",
            "pub fn total(xs: List<Number>): Number { xs.sum() }\n",
        ),
        (
            "qualified.ab",
            "pub fn tail_of(
    xs: core::collections::list::List<Number>,
): core::collections::list::List<Number> { xs.tail() }

pub fn keys_len(m: core::collections::map::Map<String, Number>): Number {
    m.keys().length()
}
",
        ),
        (
            "imported.ab",
            "use core::collections::list::List;
use core::collections::set::Set;

pub fn double_all(xs: List<Number>): List<Number> { xs.map((x: Number) => x * 2) }
pub fn set_len(s: Set<Number>): Number { s.length() }
",
        ),
        (
            "main.ab",
            r#"pub fn run(): Number {
  let doubled = pkg::imported::double_all(pkg::qualified::tail_of([1, 2, 3]));
  let m = Map::empty().insert("four", 1);
  let s = Set::empty().insert(9);
  pkg::bare::total(doubled)
    + pkg::qualified::keys_len(m)
    + pkg::imported::set_len(s)
}
"#,
        ),
    ]);
    // tail([1,2,3]) = [2,3]; doubled = [4,6]; total = 10; + 1 key + 1 element.
    assert_eq!(run(dir.path()), "12");
}

#[test]
fn map_and_set_keys_are_not_restricted_to_strings() {
    // `Map<K, V>`/`Set<T>` are declared fully generic and the checker accepts
    // any key type; the runtime must honor the same contract. A well-typed
    // program keyed by `Number`, a tuple, and a record must *run*, not crash
    // with "expected String". This is the regression that motivated the work:
    // before, only string keys survived the native boundary.
    let dir = package(&[(
        "main.ab",
        r#"pub fn run(): Number {
  // Number keys: insert replaces in place, get/remove/keys all work.
  let nums = Map::empty().insert(4, 1).insert(9, 2).insert(4, 10);
  // Tuple key.
  let tup = Map::empty().insert((1, 2), 100);
  // Record key — equal by structure regardless of field order.
  let rec = Map::empty().insert({ x: 1, y: 2 }, 1000);
  // Set of Numbers round-trips through to_list, deduping on the way.
  let set = Set::empty().insert(7).insert(7).insert(8);
  nums.length()                                  // 2
    + nums.get(4).unwrap_or(0)                   // 10
    + tup.get((1, 2)).unwrap_or(0)               // 100
    + rec.get({ y: 2, x: 1 }).unwrap_or(0)       // 1000
    + set.to_list().sum()                        // 15
    + set.length()                               // 2
}
"#,
    )]);
    // 2 + 10 + 100 + 1000 + 15 + 2 = 1129.
    assert_eq!(run(dir.path()), "1129");
}

#[test]
fn container_names_cannot_be_redeclared() {
    // The reserved-identity rule: resolution is scope-based, but declaring a
    // struct *named* `List` (without the reserved uuid, which only the core
    // declaration may carry) is an identity hijack and fails the check —
    // exactly like the primitives.
    let dir = package(&[(
        "main.ab",
        "struct List<T> { items: T }\n\npub fn run(): Number { 0 }\n",
    )]);
    let output = check_fails(dir.path());
    assert!(
        output.contains("reserved"),
        "expected a reserved-identity error, got:\n{output}"
    );
}

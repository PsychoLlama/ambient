//! Fully-qualified foreign enum-variant resolution, end-to-end through the
//! `ambient` binary.
//!
//! Pins the access rule for enum variants: a *foreign* variant reached
//! fully-qualified — with no `use` of the variant itself — resolves through
//! the canonical `Fqn` channel as both a constructor and a match pattern, and
//! wins over a same-named *local* variant by identity. This is the regression
//! that such a reference used to require a same-named local variant to resolve
//! at all (a bare-name reverse lookup deciding a fully-qualified reference),
//! silently mis-resolving to the local variant when one existed.

mod common;
use common::{ambient_cmd, temp_multi_package};

const SHAPES: &str =
    "pub unique(D098767B-4093-4D5C-BA37-AD92AA7B5D02) enum Shape { Circle(Number), Dot }\n";

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

/// Run the package at `main.ab` + `shapes.ab`, asserting success and returning
/// trimmed stdout.
fn run(main: &str) -> String {
    let (dir, pkg) = temp_multi_package(&[("shapes.ab", SHAPES), ("main.ab", main)]);
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    let stdout = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "run failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    drop(dir);
    stdout.trim().to_string()
}

#[test]
fn every_qualified_spelling_resolves_as_constructor_and_pattern() {
    // Module-qualified (`shapes::Circle`), explicit-enum (`shapes::Shape::Circle`
    // as a pattern, `pkg::shapes::Shape::Circle` as a constructor), and
    // enum-imported (`Shape::Circle` after `use pkg::shapes::Shape`) each
    // resolve — no `use` of the variant itself. (`pkg`-rooted paths are legal
    // in expression position but not in patterns, a parser limitation, so
    // patterns use the module-qualified spelling.)
    let out = run(r"
use pkg::shapes;
use pkg::shapes::Shape;

pub fn a(s: Shape): Number {
  match s { shapes::Circle(r) => r, shapes::Dot => 0 }
}

pub fn b(s: Shape): Number {
  match s { shapes::Shape::Circle(r) => r, shapes::Shape::Dot => 0 }
}

pub fn c(s: Shape): Number {
  match s { Shape::Circle(r) => r, Shape::Dot => 0 }
}

pub fn run(): Number {
  a(shapes::Circle(1))
    + b(pkg::shapes::Shape::Circle(2))
    + c(Shape::Circle(4))
}
");
    assert_eq!(out, "7");
}

#[test]
fn foreign_qualified_variant_wins_over_a_same_named_local_variant() {
    // The teeth of the fix: a local enum declares a variant `Circle` too — and,
    // to make a mis-resolution observable at runtime, at a different tag than
    // the foreign `Shape::Circle` (tag 0). A qualified reference to the foreign
    // variant must resolve to the foreign enum by identity, never to the local
    // decoy. Were the checker or compiler to fall back to the bare name, the
    // pattern would take the local `Circle`'s tag (1) and the match would miss
    // (returning 0), or the checker would report a spurious type mismatch.
    let out = run(r"
use pkg::shapes;

pub unique(D098767B-4093-4D5C-BA37-AD92AA7B5DEC) enum Decoy { Dot, Circle(Number) }

pub fn run(): Number {
  match shapes::Circle(42) {
    shapes::Circle(r) => r,
    shapes::Dot => 0,
  }
}
");
    assert_eq!(out, "42");
}

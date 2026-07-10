//! Module system: fully-qualified paths, whole-module imports, item imports, and shadowing.

mod common;
use common::*;

// ─────────────────────────────────────────────────────────────────────────────
// Module System Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_core_functions_fully_qualified() {
    // Compiled core functions (not intrinsics) callable with no import.
    // `flatten` is the surviving compiled core free function — its receiver
    // would be `Option<Option<T>>`, inexpressible in an `impl<T> Option<T>`,
    // so it stays a free function while the rest became inherent methods.
    let (dir, pkg) = temp_package(
        r"
        pub fn run(): Number {
            core::option::flatten(Some(Some(10))).unwrap_or(0)
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("10"));
    drop(dir);
}

#[test]
fn test_core_whole_module_import_alias() {
    // `use core::collections::list;` binds the alias `list` for qualified
    // access — here naming the `List` type through it.
    let (dir, pkg) = temp_package(
        r"
        use core::collections::list;

        pub fn run(): Number {
            let xs: list::List<Number> = List::range(1, 5);
            xs.fold(0, (acc: Number, x: Number) => acc + x)
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("10"));
    drop(dir);
}

#[test]
fn test_core_item_import() {
    // `use core::option::{flatten};` binds a plain name. (Collection helpers
    // are now inherent methods/associated fns, so `flatten` — a receiverless
    // core free function — is the importable plain name.)
    let (dir, pkg) = temp_package(
        r"
        use core::option::{flatten};

        pub fn run(): Number {
            flatten(Some(Some(10))).unwrap_or(0)
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("10"));
    drop(dir);
}

#[test]
fn test_non_brace_item_import() {
    // `use pkg::utils::triple;` (no braces) imports the *item* `triple`,
    // exactly like the brace form. Braces are pure grouping.
    let (dir, pkg) = temp_multi_package(&[
        (
            "main.ab",
            r"
            use pkg::utils::triple;

            pub fn run(): Number {
                triple(7) + triple(1)
            }
            ",
        ),
        (
            "utils.ab",
            r"
            pub fn triple(x: Number): Number { x * 3 }
            ",
        ),
    ]);
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("24"));
    drop(dir);
}

#[test]
fn test_non_brace_core_item_import() {
    // The unification reaches core too: `use core::option::flatten;` binds the
    // bare name `flatten` without braces.
    let (dir, pkg) = temp_package(
        r"
        use core::option::flatten;

        pub fn run(): Number {
            flatten(Some(Some(6))).unwrap_or(0)
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("6"));
    drop(dir);
}

#[test]
fn test_whole_module_user_import() {
    // `use self::utils;` then `utils::helper()` — qualified module-member
    // calls on user modules.
    let (dir, pkg) = temp_multi_package(&[
        (
            "main.ab",
            r"
            use self::utils;

            pub fn run(): Number {
                utils::triple(7) + utils::triple(1)
            }
            ",
        ),
        (
            "utils.ab",
            r"
            pub fn triple(x: Number): Number { x * 3 }
            ",
        ),
    ]);
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("24"));
    drop(dir);
}

#[test]
fn test_local_variable_shadows_module_alias() {
    // A local binding named like a module alias wins: `utils.triple` is
    // then a (failing) trait-method call on the value, not a module call.
    let (dir, pkg) = temp_multi_package(&[
        (
            "main.ab",
            r"
            use self::utils;

            pub fn run(): Number {
                let utils = 5;
                utils.triple(7)
            }
            ",
        ),
        (
            "utils.ab",
            r"
            pub fn triple(x: Number): Number { x * 3 }
            ",
        ),
    ]);
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(
        !output.status.success(),
        "shadowed alias must not resolve as a module call: {output:?}"
    );
    drop(dir);
}

#[test]
fn test_method_call_resolves_inside_perform_arguments() {
    // Regression: perform arguments used to be type-checked on
    // CLONES of the argument expressions, so resolutions recorded during
    // inference (trait method symbols, operator overloads) were silently
    // discarded and compilation failed.
    let (dir, pkg) = temp_package(
        r#"
        unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00001111) struct Point { x: Number }

        unique(AAAAAAAA-BBBB-4CCC-8DDD-000000000017) trait Doubled {
            fn doubled(self): Number;
        }

        impl Doubled for Point {
            fn doubled(self): Number { self.x * 2 }
        }

        pub fn run(): () with core::system::Stdio {
            let p = Point { x: 21 };
            core::system::Stdio::out!(core::convert::to_string(p.doubled()));
        }
        "#,
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("42"));
    drop(dir);
}

// ─────────────────────────────────────────────────────────────────────────────
// Same-named types and variants resolve to the right defining module.
//
// The resolve pass decides every nominal type head's and variant pattern's
// identity, so a same-named local declaration never captures an imported one
// (and vice versa), and a variant name shared by two enums never mis-dispatches.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_local_type_shadows_a_same_named_import() {
    // The motivating bug (structural fix): a bare type head that could name
    // either an imported type or a same-named local one resolves to the
    // *local* declaration, exactly as a bare value reference does. `main`
    // both imports `lib::Tag` and declares its own `Tag`; the bare annotation
    // `Tag` on `f` resolves to the local enum, so the local `Local` variant
    // satisfies it and the program runs.
    let (dir, pkg) = temp_multi_package(&[
        (
            "lib.ab",
            "pub unique(A1B2C3D4-0000-0000-0000-0000000000AA) enum Tag { Foreign }\n",
        ),
        (
            "main.ab",
            r"use pkg::lib::Tag;

unique(A1B2C3D4-0000-0000-0000-0000000000BB) enum Tag { Local }

// Bare `Tag` resolves to this module's own enum, not the import.
pub fn f(t: Tag): Number { match t { Local => 1 } }

pub fn run(): Number { f(Local) }
",
        ),
    ]);
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("1"));
    drop(dir);
}

#[test]
fn test_foreign_value_does_not_satisfy_a_same_named_local_type() {
    // The negative half: a value of the imported `lib::Tag` cannot flow into
    // a parameter annotated with the bare (local) `Tag`, proving the bare
    // head really resolved to the local nominal, not the imported one.
    let (dir, pkg) = temp_multi_package(&[
        (
            "lib.ab",
            "pub unique(A1B2C3D4-0000-0000-0000-0000000000AA) enum Tag { Foreign }\npub fn mk(): pkg::lib::Tag { Foreign }\n",
        ),
        (
            "main.ab",
            r"use pkg::lib;
use pkg::lib::Tag;

unique(A1B2C3D4-0000-0000-0000-0000000000BB) enum Tag { Local }
pub fn f(t: Tag): Number { match t { Local => 1 } }
pub fn run(): Number { f(pkg::lib::mk()) }
",
        ),
    ]);
    let output = ambient_cmd()
        .arg("check")
        .arg(&pkg)
        .output()
        .expect("check");
    assert!(
        !output.status.success(),
        "a foreign value must not satisfy the same-named local type: {output:?}"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Tag"),
        "expected a type mismatch between the two Tag types: {output:?}"
    );
    drop(dir);
}

#[test]
fn test_variant_pattern_binds_the_right_enum_despite_a_name_collision() {
    // Two enums in different modules share a variant name `Val`. A `match` on
    // the imported enum's value uses the module-qualified pattern `lib::Val`,
    // which the resolve pass stamps with the imported variant's two-segment
    // identity — so the checker binds the *imported* enum's variant by that
    // identity and never confuses it with the local `Val`. (Previously the
    // pattern's variant was looked up by bare name through a collision-prone
    // reverse map.)
    let (dir, pkg) = temp_multi_package(&[
        (
            "lib.ab",
            "pub unique(A1B2C3D4-0000-0000-0000-0000000000AA) enum Wrap { Val(Number) }\npub fn wrapped(): pkg::lib::Wrap { Val(5) }\n",
        ),
        (
            "main.ab",
            r"use pkg::lib;

unique(A1B2C3D4-0000-0000-0000-0000000000BB) enum Local { Val(Number) }

pub fn run(): Number {
  match pkg::lib::wrapped() {
    lib::Val(n) => n
  }
}
",
        ),
    ]);
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("5"));
    drop(dir);
}

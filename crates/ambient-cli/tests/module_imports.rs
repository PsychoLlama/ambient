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

        trait Doubled {
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

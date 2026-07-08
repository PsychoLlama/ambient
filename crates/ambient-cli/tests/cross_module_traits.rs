//! Cross-module traits, imports of nominal types, and core-library method dispatch.

mod common;
use common::*;

// ─────────────────────────────────────────────────────────────────────────────
// Cross-module traits
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_cross_module_trait_dispatch() {
    // A type, its trait impls (using the prelude Add trait and a local
    // trait), and its constructor live in one module; another module calls
    // the operator and the method. Dispatch symbols must link across the
    // module boundary. The caller must `use` the local `Doubled` trait to
    // dispatch its method — trait defs are import-scoped (only the prelude
    // operator traits are always in scope).
    let (_dir, pkg) = temp_multi_package(&[
        (
            "money.ab",
            r#"
            pub unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00001111) struct Money { cents: Number }

            impl Add for Money {
                fn add(self, other: Money): Money {
                    Money { cents: self.cents + other.cents }
                }
            }

            pub trait Doubled {
                fn doubled(self): Number;
            }

            impl Doubled for Money {
                fn doubled(self): Number {
                    self.cents * 2
                }
            }

            pub fn make(cents: Number): Money {
                Money { cents: cents }
            }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::money::{Money, make, Doubled};

            pub fn run(): Number {
                let total = make(100) + make(50);
                total.doubled() + total.cents
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("450"),
        "expected 450 in output, got: {stdout}"
    );
}

#[test]
fn test_cross_module_inherent_dispatch() {
    // Inherent methods link across module boundaries exactly like trait
    // methods: the dispatch symbol resolves by type identity, no import
    // of the impl needed.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "money.ab",
            r#"
            pub unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00003333) struct Money { cents: Number }

            impl Money {
                fn doubled(self): Number {
                    self.cents * 2
                }
                fn zero(): Money {
                    Money { cents: 0 }
                }
            }

            pub fn make(cents: Number): Money {
                Money { cents: cents }
            }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::money::{Money, make};

            pub fn run(): Number {
                let m = make(100);
                m.doubled() + Money::zero().cents
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("200"),
        "expected 200 in output, got: {stdout}"
    );
}

#[test]
fn test_cross_module_enum_import() {
    // Importing an enum brings its type, variant constructors, and
    // patterns into scope, exactly as if it were declared locally.
    // Inherent methods dispatch by uuid, so they need no import at all.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "shapes.ab",
            r#"
            pub unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00002222) enum Shape {
                Circle(Number),
                Square(Number),
                Dot,
            }

            impl Shape {
                fn area(self): Number {
                    match self {
                        Circle(r) => 3 * r * r,
                        Square(side) => side * side,
                        Dot => 0,
                    }
                }
            }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::shapes::{Shape};

            fn describe(s: Shape): Number {
                match s {
                    Circle(r) => r,
                    Square(side) => side * 2,
                    Dot => 100,
                }
            }

            pub fn run(): Number {
                describe(Circle(10)) + describe(Dot) + Circle(2).area() + Square(3).area()
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // 10 + 100 + 12 + 9
    assert!(
        stdout.contains("131"),
        "expected 131 in output, got: {stdout}"
    );
}

#[test]
fn test_cross_module_const_import() {
    // A `pub const` imported into another module inlines its literal value
    // at each reference, exactly as if it were declared locally. The value
    // travels the same AST channel as imported enums, not a hash link.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "config.ab",
            r#"
            pub const ANSWER: Number = 42;
            pub const SCALE: Number = 100;
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::config::{ANSWER, SCALE};

            pub fn run(): Number {
                ANSWER + SCALE
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // 42 + 100
    assert!(
        stdout.contains("142"),
        "expected 142 in output, got: {stdout}"
    );
}

#[test]
fn test_enum_variant_import_is_rejected() {
    // Variants don't import piecemeal — patterns and constructor tags
    // need the whole declaration in scope.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "shapes.ab",
            r#"
            pub unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00005555) enum Shape {
                Circle(Number),
                Dot,
            }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::shapes::{Circle};

            pub fn run(): Number {
                0
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    assert!(!output.status.success(), "expected variant import to fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("import its enum"),
        "expected variant-import hint, got: {stderr}"
    );
}

#[test]
fn test_foreign_nominal_type_hidden_without_import() {
    // A foreign package type is not visible by bare name unless imported:
    // constructing it without a `use` is an error, even though its module
    // is part of the same package. Trait/impl coherence stays build-global,
    // but nominal types follow the same `pub`/`use` rules as values.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "widgets.ab",
            r#"
            pub unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00004444) struct Widget { size: Number }
            "#,
        ),
        (
            "main.ab",
            r#"
            pub fn run(): Number {
                Widget { size: 42 }.size
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    assert!(
        !output.status.success(),
        "constructing an unimported foreign type must fail, stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn test_foreign_nominal_type_visible_with_import() {
    // Importing the type by name makes it constructible, just like a local
    // declaration — the `use` is what brings it into scope.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "widgets.ab",
            r#"
            pub unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00005544) struct Widget { size: Number }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::widgets::{Widget};

            pub fn run(): Number {
                Widget { size: 42 }.size
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("42"),
        "expected 42 in output, got: {stdout}"
    );
}

#[test]
fn test_imported_functions_share_foreign_nominal_identity() {
    // A value of a foreign type flows between two imported functions
    // without the caller ever naming the type: signature hydration still
    // resolves the nominal identity even though the type stays hidden from
    // bare-name use. This guards the retraction that closes the leak.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "widgets.ab",
            r#"
            pub unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00006644) struct Widget { size: Number }

            pub fn make(n: Number): Widget { Widget { size: n } }
            pub fn size_of(w: Widget): Number { w.size }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::widgets::{make, size_of};

            pub fn run(): Number {
                size_of(make(42))
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("42"),
        "expected 42 in output, got: {stdout}"
    );
}

#[test]
fn test_reserved_uuid_cannot_be_hijacked() {
    // Option's reserved uuid with a different shape must be rejected —
    // otherwise the declaration would unify with real Options and claim
    // their inherent methods.
    let (_dir, pkg) = temp_multi_package(&[(
        "main.ab",
        r#"
        unique(FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFF0001) enum MyOption<T> {
            Nothing,
            Just(T),
        }

        pub fn run(): Number {
            0
        }
        "#,
    )]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    assert!(!output.status.success(), "expected hijack to be rejected");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("reserved identity"),
        "expected reserved-identity error, got: {stderr}"
    );
}

#[test]
fn test_private_enum_is_not_importable() {
    // A bare `enum` (no `pub`) stays module-local.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "shapes.ab",
            r#"
            unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00006666) enum Secret {
                Hidden,
            }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::shapes::{Secret};

            pub fn run(): Number {
                0
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    assert!(!output.status.success(), "expected private import to fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not public"),
        "expected visibility error, got: {stderr}"
    );
}

#[test]
fn test_cross_module_duplicate_inherent_method_error() {
    // Two modules in the build closure defining the same inherent method
    // for the same type is unresolvable ambiguity: both definitions claim
    // one dispatch symbol. (Coherence is scoped to the build closure —
    // modules never loaded into a program can't collide with it.)
    let (_dir, pkg) = temp_multi_package(&[
        (
            "a.ab",
            r#"
            pub unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00004444) struct Money { cents: Number }

            impl Money {
                fn doubled(self): Number { self.cents * 2 }
            }

            pub fn make(cents: Number): Money {
                Money { cents: cents }
            }
            "#,
        ),
        (
            "b.ab",
            r#"
            use pkg::a::{Money};

            impl Money {
                fn doubled(self): Number { self.cents * 4 }
            }

            pub fn touch(): Number { 1 }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::a::{make};
            use pkg::b::{touch};

            pub fn run(): Number {
                make(touch()).doubled()
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    assert!(
        !output.status.success(),
        "duplicate cross-module inherent methods must be rejected"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("duplicate inherent method"),
        "expected duplicate inherent method error, got: {stderr}"
    );
}

#[test]
fn test_core_option_and_list_methods() {
    // The core library's Option/Result/List helpers are exposed as real
    // methods via inherent impls written in Ambient (core_lib/*.ab).
    CliTest::new(
        r#"
        pub fn run(): Number {
            let doubled = Some(20).map((v) => v * 2).unwrap_or(0);
            let empty = None.unwrap_or(2);
            let list_sum = [1, 2, 3].map((x) => x * 10).fold(0, (acc, x) => acc + x);
            let evens = [1, 2, 3, 4].filter((x) => x % 2 == 0).length();
            let chained = Ok(5).map((v) => v + 1).ok().unwrap_or(0);
            doubled + empty + list_sum + evens + chained
        }
    "#,
    )
    .expect_output("110");
}

#[test]
fn test_core_method_and_module_call_coexist() {
    // A module companion function `scaled(n, factor)` (called qualified as
    // `nums::scaled(...)`) and an inherent method `.scaled(factor)` of the
    // same name both resolve and agree. Core no longer exposes combinators
    // as free functions, so the coexistence is demonstrated with a user
    // module rather than `core::option::map`.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "nums.ab",
            r"
            pub fn scaled(n: Number, factor: Number): Number { n * factor }
            ",
        ),
        (
            "main.ab",
            r"
            use self::nums;

            unique(BBBBCCCC-DDDD-EEEE-FFFF-000011112222) struct Counter { value: Number }

            impl Counter {
                fn scaled(self, factor: Number): Number { self.value * factor }
            }

            pub fn run(): Bool {
                let via_module = nums::scaled(5, 2);
                let via_method = Counter { value: 5 }.scaled(2);
                via_module == via_method
            }
            ",
        ),
    ]);
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("true"));
}

#[test]
fn test_user_cannot_redefine_core_method() {
    // Core already defines `map` for Option; a user redefinition would
    // compete for the same dispatch symbol and is rejected.
    CliTest::new(
        r#"
        impl<T> Option<T> {
            fn map<U>(self, f: (T) -> U): Option<U> {
                None
            }
        }

        fn run(): Number { 0 }
    "#,
    )
    .expect_error("duplicate inherent method");
}

#[test]
fn test_inherent_impl_on_primitives() {
    // Primitives carry inherent methods too — their type identity is the
    // reserved lowercase head no user type can claim.
    CliTest::new(
        r#"
        impl String {
            fn shout(self): String {
                self.to_upper()
            }
        }

        impl Number {
            fn clamped(self, lo: Number, hi: Number): Number {
                self.max(lo).min(hi)
            }
        }

        pub fn run(): String {
            "hi ${(99).clamped(0, 42)} " + "there".shout()
        }
    "#,
    )
    .expect_output("hi 42 THERE");
}

#[test]
fn test_string_concat_operator_at_runtime() {
    // `+` on two strings concatenates (the checker has always admitted
    // this; the VM used to reject it at runtime).
    CliTest::new(
        r#"
        pub fn run(): String {
            let name = "world";
            "hello " + name + "!"
        }
    "#,
    )
    .expect_output("hello world!");
}

#[test]
fn test_user_can_extend_core_type_with_new_method() {
    // New method names on core types are fair game — extension without
    // collision.
    CliTest::new(
        r#"
        impl<T> Option<T> {
            fn to_list(self): List<T> {
                match self {
                    Some(v) => [v],
                    None => [],
                }
            }
        }

        pub fn run(): Number {
            Some(7).to_list().length() + None.to_list().length()
        }
    "#,
    )
    .expect_output("1");
}

#[test]
fn test_prelude_traits_no_import_needed() {
    // The operator traits are prelude: an impl can reference Add without
    // any use statement or local trait declaration.
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00002222) struct Meters { value: Number }

        impl Add for Meters {
            fn add(self, other: Meters): Meters {
                Meters { value: self.value + other.value }
            }
        }

        fn run(): Number {
            let d = Meters { value: 3 } + Meters { value: 4 };
            d.value
        }
    "#,
    )
    .expect_output("7");
}

#[test]
fn test_zero_parameter_lambda() {
    // `()` must parse as a unit literal except when followed by `=>`,
    // where it begins a zero-parameter lambda.
    CliTest::new(
        r#"
        fn call_thunk(f: () -> Number): Number {
            f()
        }

        fn run(): Number {
            let t = () => 42;
            let a = t();
            let b = call_thunk(() => { let x = 7; x * 2 });
            let unit_still_works = ();
            a + b
        }
    "#,
    )
    .expect_output("56");
}

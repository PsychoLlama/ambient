//! Enum constructors and match tests.

mod common;
use common::*;

// ─────────────────────────────────────────────────────────────────────────────
// Enum Constructor & Match Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_user_enum_construct_and_match() {
    let (dir, pkg) = temp_package(
        r"
        unique(C1B2C3D4-0000-0000-0000-000000000011) enum Shape { Circle(Number), Square(Number), Dot }

        pub fn run(): Number {
            area(Circle(2)) + area(Square(3)) + area(Dot)
        }

        fn area(s: Shape): Number {
            match s {
                Circle(r) => 3 * r * r,
                Square(side) => side * side,
                Dot => 0,
            }
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("21"));
    drop(dir);
}

#[test]
fn test_bare_enum_requires_unique() {
    // Every enum must carry a `unique(<uuid>)` prefix; a bare `enum` is
    // rejected so structurally identical enums can never be conflated.
    CliTest::new(
        r"
        enum Color { Red, Green, Blue }

        pub fn run(): Number { 0 }
        ",
    )
    .check()
    .expect_error("unique");
}

#[test]
fn test_distinct_enums_are_not_interchangeable() {
    // Two enums with identical shape are distinct nominal types: a value of
    // one cannot stand in for the other. This is the whole point of enum
    // nominal identity — shape no longer implies interchangeability.
    CliTest::new(
        r"
        unique(C1B2C3D4-0000-0000-0000-000000000020) enum Meters { M(Number) }
        unique(C1B2C3D4-0000-0000-0000-000000000021) enum Feet { F(Number) }

        fn meters_value(x: Meters): Number {
            match x { M(v) => v }
        }

        pub fn run(): Number {
            meters_value(F(3))
        }
        ",
    )
    .check()
    .expect_error("type mismatch");
}

#[test]
fn test_duplicate_inherent_method_on_enum_error() {
    // Coherence holds for enums exactly as for nominal types: a second
    // definition of a method name for the same enum is rejected because both
    // would claim the one `<uuid>::method` dispatch symbol.
    CliTest::new(
        r#"
        unique(C1B2C3D4-0000-0000-0000-000000000022) enum Toggle { On, Off }

        impl Toggle {
            fn flipped(self): Toggle {
                match self { On => Off, Off => On }
            }
        }

        impl Toggle {
            fn flipped(self): Toggle {
                self
            }
        }

        pub fn run(): Number { 0 }
    "#,
    )
    .check()
    .expect_error("duplicate inherent method");
}

#[test]
fn test_generic_nominal_enum_roundtrips() {
    // A generic `unique(...) enum` carries its type argument through
    // construction, matching, and an inherent method that returns the
    // payload — proving the nominal identity survives substitution.
    let (dir, pkg) = temp_package(
        r"
        unique(C1B2C3D4-0000-0000-0000-000000000023) enum Box<T> { Full(T), Empty }

        impl<T> Box<T> {
            fn get_or(self, fallback: T): T {
                match self {
                    Full(v) => v,
                    Empty => fallback,
                }
            }
        }

        pub fn run(): Number {
            Full(40).get_or(0) + Empty.get_or(2)
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("42"));
    drop(dir);
}

#[test]
fn test_enum_payload_is_another_nominal_enum() {
    // A variant payload written as another declared enum resolves to that
    // enum's nominal identity, so a method call on the extracted binding
    // dispatches on the payload enum's uuid — not its head name.
    let (dir, pkg) = temp_package(
        r"
        unique(D1B2C3D4-0000-0000-0000-000000000001) enum Inner { Val(Number) }
        unique(D1B2C3D4-0000-0000-0000-000000000002) enum Outer { Wrap(Inner) }

        impl Inner {
            fn doubled(self): Number {
                match self { Val(v) => v * 2 }
            }
        }

        pub fn run(): Number {
            match Wrap(Val(21)) {
                Wrap(inner) => inner.doubled(),
            }
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("42"));
    drop(dir);
}

#[test]
fn test_option_constructors_and_core_helpers() {
    let (dir, pkg) = temp_package(
        r"
        pub fn run(): Number {
            let doubled = Some(20).map((x: Number) => x * 2);
            doubled.unwrap_or(0)
                + nothing().map((x: Number) => x).unwrap_or(2)
        }

        fn nothing(): Option<Number> {
            None
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("42"));
    drop(dir);
}

#[test]
fn test_result_constructors_and_chaining() {
    let (dir, pkg) = temp_package(
        r#"
        pub fn run(): String {
            let ok = parse(5).map((x: Number) => x * 10);
            let err = parse(0 - 3);
            match ok.and_then((x: Number) => parse(x)) {
                Ok(v) => core::primitives::String::from_number(v),
                Err(e) => e,
            }
        }

        fn parse(n: Number): Result<Number, String> {
            if n > 0 { Ok(n) } else { Err("negative") }
        }
        "#,
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("50"));
    drop(dir);
}

#[test]
fn test_match_takes_correct_arm() {
    // Regression: the pattern compiler's success path used to jump straight
    // to the fail target, so every variant arm skipped its own body.
    let (dir, pkg) = temp_package(
        r"
        pub fn run(): Number {
            let hit = match Some(41) {
                Some(v) => v,
                None => 0 - 1,
            };
            let miss = match nothing() {
                Some(v) => v,
                None => 100,
            };
            hit + miss
        }

        fn nothing(): Option<Number> {
            None
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("141"));
    drop(dir);
}

#[test]
fn test_unknown_variant_pattern_is_error() {
    let (dir, pkg) = temp_package(
        r"
        pub fn run(): Number {
            match Some(1) {
                Sume(v) => v,
                None => 0,
            }
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown enum variant"),
        "expected unknown-variant error: {output:?}"
    );
    drop(dir);
}

#[test]
fn test_variant_payload_mismatch_is_error() {
    let (dir, pkg) = temp_package(
        r"
        pub fn run(): Number {
            match Some(1) {
                Some => 1,
                None => 0,
            }
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("payload"),
        "expected payload mismatch error: {output:?}"
    );
    drop(dir);
}

#[test]
fn test_lowercase_pattern_still_binds() {
    // Only uppercase-initial bare identifiers are variant patterns.
    let (dir, pkg) = temp_package(
        r"
        pub fn run(): Number {
            match 42 {
                x => x,
            }
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("42"));
    drop(dir);
}

//! Associated types on traits: `type Error;` declarations, per-impl
//! `type Error = ...;` bindings, `Self::X` projections in trait method
//! signatures (eliminated by the dispatching impl's binding at concrete
//! call sites, rigid on generic receivers), and the declaration-site
//! rejections (missing/unknown/duplicate bindings, undeclared `Self::X`,
//! inherent-impl `type` items).

mod common;
use common::*;

// ─────────────────────────────────────────────────────────────────────────────
// Concrete dispatch
// ─────────────────────────────────────────────────────────────────────────────

/// A concrete associated call resolves `Self::Error` to the impl's binding:
/// the call's result is a fully concrete `Result<Money, String>`.
#[test]
fn assoc_type_resolves_through_an_associated_call() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000701) trait TryParse<T> {
          type Error;
          fn try_parse(value: T): Result<Self, Self::Error>;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000702) struct Money { cents: Number }
        impl TryParse<Number> for Money {
          type Error = String;
          fn try_parse(value: Number): Result<Money, String> {
            if value < 0 { Err("negative") } else { Ok(Money { cents: value }) }
          }
        }
        fn run(): Number {
          match Money::try_parse(41) {
            Ok(m) => m.cents + 1,
            Err(msg) => msg.length(),
          }
        }
    "#,
    )
    .expect_output("42");
}

/// The error side of the projection is the binding too: a failing parse
/// hands the body the bound error type, usable as that concrete type.
#[test]
fn assoc_type_error_side_is_the_bound_type() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000703) trait TryParse<T> {
          type Error;
          fn try_parse(value: T): Result<Self, Self::Error>;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000704) struct Money { cents: Number }
        impl TryParse<Number> for Money {
          type Error = String;
          fn try_parse(value: Number): Result<Money, String> {
            if value < 0 { Err("negative") } else { Ok(Money { cents: value }) }
          }
        }
        fn run(): String {
          match Money::try_parse(0 - 5) {
            Ok(m) => "ok",
            Err(msg) => msg,
          }
        }
    "#,
    )
    .expect_output("negative");
}

/// Two argument-differing impls bind the associated type differently; the
/// argument-directed selection picks the impl and with it the error type.
#[test]
fn assoc_type_follows_argument_directed_impl_selection() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000705) trait TryParse<T> {
          type Error;
          fn try_parse(value: T): Result<Self, Self::Error>;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000706) struct Money { cents: Number }
        impl TryParse<Number> for Money {
          type Error = String;
          fn try_parse(value: Number): Result<Money, String> { Ok(Money { cents: value }) }
        }
        impl TryParse<String> for Money {
          type Error = Number;
          fn try_parse(value: String): Result<Money, Number> { Err(value.length()) }
        }
        fn run(): Number {
          let a = match Money::try_parse(40) { Ok(m) => m.cents, Err(s) => s.length() };
          let b = match Money::try_parse("xy") { Ok(m) => m.cents, Err(n) => n };
          a + b
        }
    "#,
    )
    .expect_output("42");
}

/// An instance method can reference the projection (`fn tag(self):
/// Self::Output`), resolved per impl by dot dispatch.
#[test]
fn assoc_type_resolves_through_instance_dispatch() {
    CliTest::new(
        r#"
        use core::convert::to_string;
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000707) trait Encode {
          type Output;
          fn encode(self): Self::Output;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000708) struct Money { cents: Number }
        impl Encode for Money {
          type Output = String;
          fn encode(self): String { "$" + to_string(self.cents) }
        }
        fn run(): String { Money { cents: 7 }.encode() }
    "#,
    )
    .expect_output("$7");
}

// ─────────────────────────────────────────────────────────────────────────────
// Generic receivers: the projection stays rigid
// ─────────────────────────────────────────────────────────────────────────────

/// In a bounded generic body the impl is unknown, so `x.encode()` types as
/// the opaque projection `T::Output`; the body can carry it around but a
/// concrete caller still gets its concrete type.
#[test]
fn assoc_projection_is_opaque_in_generic_bodies() {
    CliTest::new(
        r#"
        use core::convert::to_string;
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000709) trait Encode {
          type Output;
          fn encode(self): Self::Output;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000070A) struct Money { cents: Number }
        impl Encode for Money {
          type Output = String;
          fn encode(self): String { to_string(self.cents) }
        }
        fn encode_twice<T: Encode>(a: T, b: T): Number {
          let x = a.encode();
          let y = b.encode();
          2
        }
        fn run(): Number { encode_twice(Money { cents: 1 }, Money { cents: 2 }) }
    "#,
    )
    .expect_output("2");
}

/// The opaque projection unifies only with itself: treating `T::Output` as
/// a concrete type inside the generic body is a compile error.
#[test]
fn assoc_projection_does_not_unify_with_a_concrete_type() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000070B) trait Encode {
          type Output;
          fn encode(self): Self::Output;
        }
        fn stringify<T: Encode>(x: T): String {
          x.encode()
        }
        fn run(): Number { 0 }
    "#,
    )
    .check()
    .expect_error("T::Output");
}

// ─────────────────────────────────────────────────────────────────────────────
// Declaration-site rules
// ─────────────────────────────────────────────────────────────────────────────

/// An impl must bind every associated type its trait declares.
#[test]
fn impl_missing_assoc_binding_is_rejected() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000070C) trait Encode {
          type Output;
          fn encode(self): Self::Output;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000070D) struct Money { cents: Number }
        impl Encode for Money {
          fn encode(self): String { "x" }
        }
        fn run(): Number { 0 }
    "#,
    )
    .check()
    .expect_error("does not bind its associated type `Output`");
}

/// An impl may not bind a name the trait does not declare.
#[test]
fn impl_unknown_assoc_binding_is_rejected() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000070E) trait Encode {
          type Output;
          fn encode(self): Self::Output;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000070F) struct Money { cents: Number }
        impl Encode for Money {
          type Output = String;
          type Extra = Number;
          fn encode(self): String { "x" }
        }
        fn run(): Number { 0 }
    "#,
    )
    .check()
    .expect_error("binds `type Extra`");
}

/// An impl method must agree with the trait signature *under the binding*:
/// returning a type other than the bound one is a conformance error.
#[test]
fn impl_method_must_match_the_assoc_binding() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000710) trait Encode {
          type Output;
          fn encode(self): Self::Output;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000711) struct Money { cents: Number }
        impl Encode for Money {
          type Output = String;
          fn encode(self): Number { self.cents }
        }
        fn run(): Number { 0 }
    "#,
    )
    .check()
    .expect_error("in impl method `encode`");
}

/// A `Self::X` the trait never declares is an undefined type name.
#[test]
fn undeclared_self_projection_is_rejected() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000712) trait Encode {
          type Output;
          fn encode(self): Self::Wrong;
        }
        fn run(): Number { 0 }
    "#,
    )
    .check()
    .expect_error("Self::Wrong");
}

/// Declaring the same associated type twice is a declaration error.
#[test]
fn duplicate_assoc_declaration_is_rejected() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000713) trait Encode {
          type Output;
          type Output;
          fn encode(self): Self::Output;
        }
        fn run(): Number { 0 }
    "#,
    )
    .check()
    .expect_error("declares the associated type `Output` twice");
}

/// Inherent impls have no trait to declare associated types; a `type` item
/// there is rejected.
#[test]
fn inherent_impl_assoc_binding_is_rejected() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000714) struct Money { cents: Number }
        impl Money {
          type Output = String;
          fn double(self): Money { Money { cents: self.cents * 2 } }
        }
        fn run(): Number { 0 }
    "#,
    )
    .check()
    .expect_error("binds `type Output`");
}

// ─────────────────────────────────────────────────────────────────────────────
// Cross-module: foreign impls carry their bindings
// ─────────────────────────────────────────────────────────────────────────────

/// A trait with an associated type declared in one module, implemented in a
/// second, dispatched from a third: the foreign-impl registration carries
/// the binding, so the projection resolves identically everywhere.
#[test]
fn assoc_bindings_travel_across_modules() {
    use std::fs;
    use std::process::Command;

    let dir = tempfile::TempDir::new().expect("create temp dir");
    fs::write(
        dir.path().join("ambient.toml"),
        "[package]\nname = \"assoc\"\nversion = \"0.1.0\"\n",
    )
    .expect("write manifest");
    let src = dir.path().join("src");
    fs::create_dir_all(&src).expect("create src");
    fs::write(
        src.join("proto.ab"),
        r"
        pub unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000901) trait TryParse<T> {
          type Error;
          fn try_parse(value: T): Result<Self, Self::Error>;
        }
        ",
    )
    .expect("write proto");
    fs::write(
        src.join("money.ab"),
        r#"
        use pkg::proto::TryParse;
        pub unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000902) struct Money { cents: Number }
        impl TryParse<Number> for Money {
          type Error = String;
          fn try_parse(value: Number): Result<Money, String> {
            if value < 0 { Err("negative") } else { Ok(Money { cents: value }) }
          }
        }
        "#,
    )
    .expect("write money");
    fs::write(
        src.join("main.ab"),
        r"
        use pkg::money::Money;
        pub fn run(): Number {
          match Money::try_parse(42) {
            Ok(m) => m.cents,
            Err(e) => e.length(),
          }
        }
        ",
    )
    .expect("write main");

    let output = Command::new(env!("CARGO_BIN_EXE_ambient"))
        .arg("run")
        .arg(dir.path())
        .output()
        .expect("spawn ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "run failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("42"), "unexpected output:\n{stdout}");
}

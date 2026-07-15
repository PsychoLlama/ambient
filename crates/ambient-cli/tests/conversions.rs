//! The `From<T>`/`Into<T>` conversion traits and trait-level type
//! parameters: argument-directed impl selection (`Money::from(x)`),
//! result-type-directed selection (`x.into()`, immediate and deferred),
//! the From-satisfies-Into bridge in bounds, conditional `From` impls,
//! and the declaration-site rejections (headless trait arguments, arity
//! mismatches, argument-head coherence).

mod common;
use common::*;

// ─────────────────────────────────────────────────────────────────────────────
// From: associated calls
// ─────────────────────────────────────────────────────────────────────────────

/// `impl From<Number> for Money` (prelude `From`, no import) constructs via
/// the associated call `Money::from(5)`.
#[test]
fn from_impl_dispatches_an_associated_call() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000601) struct Money { cents: Number }
        impl From<Number> for Money {
          fn from(value: Number): Money { Money { cents: value * 100 } }
        }
        fn run(): Number { Money::from(5).cents }
    "#,
    )
    .expect_output("500");
}

/// Two argument-differing `From` impls coexist for one type (coherence keys
/// on the argument's head), and `Money::from(x)` selects by the argument's
/// type.
#[test]
fn multiple_from_impls_select_by_argument_type() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000602) struct Money { cents: Number }
        impl From<Number> for Money {
          fn from(value: Number): Money { Money { cents: value * 100 } }
        }
        impl From<String> for Money {
          fn from(value: String): Money { Money { cents: value.length() } }
        }
        fn run(): Number { Money::from(5).cents + Money::from("ab").cents }
    "#,
    )
    .expect_output("502");
}

/// A conditional `From` impl (`impl<T> From<List<T>> for Wrap<T>`): the
/// trait argument carries the impl's parameter, the argument match binds
/// it, and the associated call instantiates the generic target.
#[test]
fn conditional_from_impl_binds_parameters_from_the_argument() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000603) struct Wrap<T> { items: List<T> }
        impl<T> From<List<T>> for Wrap<T> {
          fn from(value: List<T>): Wrap<T> { Wrap { items: value } }
        }
        fn run(): Number {
          Wrap::from([1, 2, 3]).items.fold(0, (acc, x) => acc + x)
        }
    "#,
    )
    .expect_output("6");
}

// ─────────────────────────────────────────────────────────────────────────────
// Into: the From bridge
// ─────────────────────────────────────────────────────────────────────────────

/// `x.into()` with a single `From` impl for the receiver resolves
/// immediately — no `Into` impl exists anywhere; the bridge supplies it.
#[test]
fn into_resolves_through_a_from_impl() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000604) struct Money { cents: Number }
        impl From<Number> for Money {
          fn from(value: Number): Money { Money { cents: value * 100 } }
        }
        fn run(): Number {
          let n = 7;
          let m: Money = n.into();
          m.cents
        }
    "#,
    )
    .expect_output("700");
}

/// Two `From` impls make `n.into()` ambiguous at the call site; selection
/// defers until the body settles and the annotation picks the target.
#[test]
fn into_defers_selection_to_the_annotated_target() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000605) struct Money { cents: Number }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000606) struct Tag { id: Number }
        impl From<Number> for Money {
          fn from(value: Number): Money { Money { cents: value * 100 } }
        }
        impl From<Number> for Tag {
          fn from(value: Number): Tag { Tag { id: value + 1 } }
        }
        fn run(): Number {
          let n = 3;
          let m: Money = n.into();
          let t: Tag = n.into();
          m.cents + t.id
        }
    "#,
    )
    .expect_output("304");
}

/// A deferred `x.into()` whose target nothing pins is a clear "add an
/// annotation" error, never a miscompile.
#[test]
fn into_with_no_target_reports_annotation_hint() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000607) struct Money { cents: Number }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000608) struct Tag { id: Number }
        impl From<Number> for Money {
          fn from(value: Number): Money { Money { cents: value } }
        }
        impl From<Number> for Tag {
          fn from(value: Number): Tag { Tag { id: value } }
        }
        fn run(): Number {
          let n = 3;
          let x = n.into();
          0
        }
    "#,
    )
    .expect_error("add an annotation");
}

/// `T: Into<Money>` in a bound: the body dispatches `x.into()` through the
/// dictionary, and a concrete call site satisfies the bound through the
/// receiver's `From` impl (the bridge — a `From` dictionary *is* an `Into`
/// dictionary).
#[test]
fn into_bound_is_satisfied_by_a_from_impl() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000609) struct Money { cents: Number }
        impl From<Number> for Money {
          fn from(value: Number): Money { Money { cents: value * 100 } }
        }
        fn pay<T: Into<Money>>(x: T): Number { x.into().cents }
        fn run(): Number { pay(9) }
    "#,
    )
    .expect_output("900");
}

/// A generic caller forwards its own `Into` dictionary into another bounded
/// generic — the bound's trait argument travels with the dictionary.
#[test]
fn into_bound_forwards_through_generic_callers() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000060A) struct Money { cents: Number }
        impl From<Number> for Money {
          fn from(value: Number): Money { Money { cents: value + 1 } }
        }
        fn pay<T: Into<Money>>(x: T): Number { x.into().cents }
        fn indirect<T: Into<Money>>(x: T): Number { pay(x) }
        fn run(): Number { indirect(41) }
    "#,
    )
    .expect_output("42");
}

/// A direct `Into` impl (no `From` anywhere) also works — the bridge is a
/// fallback, not a replacement.
#[test]
fn direct_into_impl_dispatches() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000060B) struct Money { cents: Number }
        impl Into<Number> for Money {
          fn into(self): Number { self.cents }
        }
        fn run(): Number {
          let m = Money { cents: 250 };
          let n: Number = m.into();
          n
        }
    "#,
    )
    .expect_output("250");
}

// ─────────────────────────────────────────────────────────────────────────────
// User-defined generic traits
// ─────────────────────────────────────────────────────────────────────────────

/// Trait-level type parameters are a general feature: a user trait with two
/// argument-differing impls dispatches a zero-argument method by the
/// result type, exactly like `into`.
#[test]
fn user_generic_trait_selects_by_result_type() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000060C) trait Unwrap<T> {
          fn unwrap(self): T;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000060D) struct Money { cents: Number }
        impl Unwrap<Number> for Money {
          fn unwrap(self): Number { self.cents }
        }
        impl Unwrap<String> for Money {
          fn unwrap(self): String { "money" }
        }
        fn run(): Number {
          let m = Money { cents: 5 };
          let n: Number = m.unwrap();
          let s: String = m.unwrap();
          n + s.length()
        }
    "#,
    )
    .expect_output("10");
}

/// A generic trait's bound with an argument (`T: Unwrap<Number>`) dispatches
/// through the dictionary, and only an impl at the *same* argument satisfies
/// the call site.
#[test]
fn generic_trait_bound_matches_arguments() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000060E) trait Unwrap<T> {
          fn unwrap(self): T;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000060F) struct Money { cents: Number }
        impl Unwrap<Number> for Money {
          fn unwrap(self): Number { self.cents }
        }
        fn total<T: Unwrap<Number>>(a: T, b: T): Number { a.unwrap() + b.unwrap() }
        fn run(): Number { total(Money { cents: 2 }, Money { cents: 3 }) }
    "#,
    )
    .expect_output("5");
}

/// An impl at a different argument does not satisfy the bound.
#[test]
fn generic_trait_bound_rejects_a_mismatched_argument() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000610) trait Unwrap<T> {
          fn unwrap(self): T;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000611) struct Money { cents: Number }
        impl Unwrap<String> for Money {
          fn unwrap(self): String { "money" }
        }
        fn total<T: Unwrap<Number>>(a: T): Number { a.unwrap() }
        fn run(): Number { total(Money { cents: 2 }) }
    "#,
    )
    .expect_error("Unwrap<Number>` is not satisfied");
}

// ─────────────────────────────────────────────────────────────────────────────
// Declaration-site rules
// ─────────────────────────────────────────────────────────────────────────────

/// An impl's trait argument must have a nominal head — a bare impl
/// parameter (`impl<T> Conv<T> for M`) would be a blanket impl and is
/// rejected.
#[test]
fn headless_trait_argument_is_rejected() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000612) trait Conv<T> {
          fn conv(value: T): Self;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000613) struct M { v: Number }
        impl<T> Conv<T> for M {
          fn conv(value: T): M { M { v: 1 } }
        }
    "#,
    )
    .check()
    .expect_error("no nominal identity");
}

/// A reference to a parameterized trait must spell the right number of
/// arguments — both too few (`impl Conv for M`) and too many
/// (`T: Plain<Number>`).
#[test]
fn trait_argument_arity_is_checked() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000614) trait Conv<T> {
          fn conv(value: T): Self;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000615) struct M { v: Number }
        impl Conv for M {
          fn conv(value: Number): M { M { v: value } }
        }
    "#,
    )
    .check()
    .expect_error("takes 1 type argument(s), but 0 were spelled");
}

/// Two impls whose trait arguments share a head conflict — coherence is
/// head-granular, exactly like impl targets.
#[test]
fn same_head_trait_arguments_conflict() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000616) trait Conv<T> {
          fn conv(value: T): Self;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000617) struct M { v: Number }
        impl Conv<List<Number>> for M {
          fn conv(value: List<Number>): M { M { v: 1 } }
        }
        impl Conv<List<String>> for M {
          fn conv(value: List<String>): M { M { v: 2 } }
        }
    "#,
    )
    .check()
    .expect_error("duplicate");
}

// ─────────────────────────────────────────────────────────────────────────────
// Core-library conversions
// ─────────────────────────────────────────────────────────────────────────────

/// The core library's own `From` impls — `From<Number> for String` and
/// `From<String> for Binary`, both defined in `core` and prelude-exported —
/// are usable as plain associated calls from a user package. This pins the
/// cross-module foreign-impl registration path: the impls live in core, the
/// caller here.
#[test]
fn core_from_impls_convert_primitives() {
    CliTest::new(
        r#"
        fn run(): Number {
          let s: String = String::from(42);
          let b: Binary = Binary::from("hi");
          s.length() + b.length()
        }
    "#,
    )
    .expect_output("4");
}

/// `value.into()` resolves to the core `From<Number> for String` impl when a
/// `let: String` annotation pins the target — the everyday spelling for
/// building a string from a number through a variable.
#[test]
fn into_selects_core_string_from_impl_via_annotation() {
    CliTest::new(
        r#"
        fn run(): Number {
          let n = 42;
          let s: String = n.into();
          s.length()
        }
    "#,
    )
    .expect_output("2");
}

/// A declaration claiming the reserved `From` uuid must match the canonical
/// shape — the same hijack guard the operator traits get.
#[test]
fn reserved_from_shape_is_pinned() {
    CliTest::new(
        r#"
        unique(FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFF0019) trait From {
          fn from(value: Number): Self;
        }
    "#,
    )
    .check()
    .expect_error("reserved identity");
}

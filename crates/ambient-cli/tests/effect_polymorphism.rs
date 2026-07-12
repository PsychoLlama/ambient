//! Effect polymorphism (Phase 1): ordinary functions generic over an
//! ability (row) variable declared `E!`. `with E` refers to that variable in
//! the function's own `with` clause and in function-type annotations; the
//! variable propagates a callee's effects to the caller and instantiates
//! independently per call site. Effects are erased before compilation, so
//! these are all check-time (and one run-time) properties.

mod common;

use common::CliTest;

/// The canonical example: an `E!`-polymorphic function forwards an effectful
/// thunk. A caller passing an effectful lambda must itself declare (or
/// handle) the ability; a caller passing a pure lambda needs nothing.
#[test]
fn effect_variable_propagates_and_instantiates_pure() {
    CliTest::new(
        r#"
        pub fn example<E!>(t: () -> String with E): String with E {
          t()
        }

        pub fn effectful_caller(): String with core::system::Stdio {
          example(() => { core::system::Stdio::out!("hi"); "done" })
        }

        pub fn pure_caller(): String {
          example(() => "pure")
        }
        "#,
    )
    .check()
    .expect_success();
}

/// A pure caller passing a pure lambda: `E` instantiates to the empty row, so
/// the whole call is pure and requires no abilities.
#[test]
fn pure_lambda_leaves_the_caller_pure() {
    CliTest::new(
        r#"
        pub fn run_thunk<E!>(t: () -> Number with E): Number with E {
          t()
        }

        pub fn total(): Number {
          run_thunk(() => 41)
        }
        "#,
    )
    .check()
    .expect_success();
}

/// Handling at the call boundary discharges the ability, so the enclosing
/// function is pure — and the program actually runs.
#[test]
fn handling_at_the_call_boundary_discharges_the_ability() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000E1) ability Beep {
          fn beep(): Number { 0 }
        }

        pub fn example<E!>(t: () -> Number with E): Number with E {
          t()
        }

        pub fn run(): Number {
          with { Beep::beep() => resume(5) } handle example(() => Beep::beep!())
        }
        "#,
    )
    .expect_output("5");
}

/// A mixed row `with Stdio, E`: the body may perform the concrete `Stdio`
/// ability *and* call the `E`-carrying parameter.
#[test]
fn mixed_concrete_and_variable_row() {
    CliTest::new(
        r#"
        pub fn logged<E!>(t: () -> String with E): String with core::system::Stdio, E {
          core::system::Stdio::out!("before");
          t()
        }
        "#,
    )
    .check()
    .expect_success();
}

/// A body performing an ability outside its declared row (`with E` only, body
/// performs `Stdio` directly) is rejected — the concrete effect escapes.
#[test]
fn ability_outside_the_declared_row_is_rejected() {
    CliTest::new(
        r#"
        pub fn leaky<E!>(t: () -> String with E): String with E {
          core::system::Stdio::out!("leak");
          t()
        }
        "#,
    )
    .check()
    .expect_error("uses ability `Stdio` but doesn't declare it");
}

/// Two ability variables may be declared and each is independently usable in
/// its own function-type position (the single-tail representation means they
/// never share one row).
#[test]
fn two_ability_variables_coexist() {
    CliTest::new(
        r#"
        pub fn use_e<E!, F!>(a: () -> Number with E): Number with E { a() }
        pub fn use_f<E!, F!>(a: () -> Number with F): Number with F { a() }
        "#,
    )
    .check()
    .expect_success();
}

/// Two ability variables in one `with` row is an error — a row has a single
/// polymorphic tail.
#[test]
fn two_variables_in_one_row_is_rejected() {
    CliTest::new(
        r#"
        pub fn bad<E!, F!>(a: () -> Number with E): Number with E, F {
          a()
        }
        "#,
    )
    .check()
    .expect_error("at most one ability variable");
}

/// Trait bounds on an ability variable are rejected — it names an effect row,
/// not a type.
#[test]
fn bounds_on_an_ability_variable_are_rejected() {
    CliTest::new(
        r#"
        pub fn bad<E!: core::cmp::Eq>(a: () -> Number with E): Number with E { a() }
        "#,
    )
    .check()
    .expect_error("cannot have trait bounds");
}

/// Using an ability variable where a type is expected is an error.
#[test]
fn ability_variable_used_as_a_type_is_rejected() {
    CliTest::new(
        r#"
        pub fn bad<E!>(x: E): Number with E { 0 }
        "#,
    )
    .check()
    .expect_error("is an ability variable, not a type");
}

/// An effect-polymorphic function calling another effect-polymorphic
/// function: the row variable flows through the call unchanged.
#[test]
fn row_variable_flows_through_a_polymorphic_call() {
    CliTest::new(
        r#"
        pub fn inner<E!>(t: () -> Number with E): Number with E { t() }
        pub fn outer<E!>(t: () -> Number with E): Number with E { inner(t) }
        "#,
    )
    .check()
    .expect_success();
}

/// A self-recursive effect-polymorphic function: the recursive call carries
/// the same row variable.
#[test]
fn self_recursive_effect_polymorphic_function() {
    CliTest::new(
        r#"
        pub fn repeat<E!>(n: Number, t: () -> Number with E): Number with E {
          if n == 0 { t() } else { repeat(n - 1, t) }
        }
        "#,
    )
    .check()
    .expect_success();
}

/// Two call sites of the same polymorphic function instantiate independently
/// — one pure, one effectful — within a single body.
#[test]
fn independent_instantiation_at_two_call_sites() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000E2) ability Beep {
          fn beep(): Number { 0 }
        }

        pub fn id<E!>(t: () -> Number with E): Number with E { t() }

        pub fn mix(): Number with Beep {
          let pure = id(() => 1);
          let effectful = id(() => Beep::beep!());
          pure + effectful
        }
        "#,
    )
    .check()
    .expect_success();
}

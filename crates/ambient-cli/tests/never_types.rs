//! Never-type (`!`) semantics, end to end.
//!
//! A never-typed expression adopts the surrounding context's type (bottom
//! elimination: the value can never exist, so the use site is
//! unreachable), while annotations stay strict — a body must genuinely
//! diverge to check against a declared `!`. Never-returning ability
//! methods are catch-only: the perform site unwinds to the handler, arms
//! cannot `resume`, and the method may omit its default implementation
//! (an unhandled perform is then a runtime fault).

mod common;

use common::CliTest;

#[test]
fn test_throw_in_value_position_adopts_the_branch_type() {
    // The motivating example: `Exception::throw!` returns `!`, which must
    // unify with the `Number` the other branch produces.
    CliTest::new(
        r#"
        fn clamp(value: Number): Number with Exception {
            if (value > 0) {
                value + 1
            } else {
                Exception::throw!("Too low.")
            }
        }

        pub fn run(): Number {
            with {
                Exception::throw(msg) => 0 - 1
            } handle clamp(41)
        }
        "#,
    )
    .expect_output("42");
}

#[test]
fn test_throw_branch_unwinds_to_the_handler_value() {
    // Same shape, unhappy path: the throw unwinds out of `clamp` and the
    // arm's value becomes the handle expression's result.
    CliTest::new(
        r#"
        fn clamp(value: Number): Number with Exception {
            if (value > 0) {
                value + 1
            } else {
                Exception::throw!("Too low.")
            }
        }

        pub fn run(): Number {
            with {
                Exception::throw(msg) => 0 - 1
            } handle clamp(0 - 5)
        }
        "#,
    )
    .expect_output("-1");
}

#[test]
fn test_throw_as_function_tail_value() {
    // A `!` tail expression checks against any declared return type.
    CliTest::new(
        r#"
        fn fail(): Number with Exception {
            Exception::throw!("nope")
        }

        pub fn run(): Number {
            with {
                Exception::throw(msg) => 7
            } handle fail()
        }
        "#,
    )
    .expect_output("7");
}

#[test]
fn test_throw_in_if_without_else() {
    // An else-less `if` requires `()` from its branch; `!` adopts that too.
    // The `if` stands in statement position with no semicolon — block-bodied
    // expressions need none when more code follows.
    CliTest::new(
        r#"
        fn guard(x: Number): Number with Exception {
            if (x < 0) {
                Exception::throw!("negative")
            }
            x * 2
        }

        pub fn run(): Number {
            with {
                Exception::throw(msg) => 0
            } handle guard(21)
        }
        "#,
    )
    .expect_output("42");
}

#[test]
fn test_throw_in_match_arm() {
    // Match arms unify against one result type; a throwing arm adopts it.
    CliTest::new(
        r#"
        fn describe(x: Number): String with Exception {
            match x {
                0 => "zero",
                _ => Exception::throw!("not zero"),
            }
        }

        pub fn run(): String {
            with {
                Exception::throw(msg) => msg
            } handle describe(1)
        }
        "#,
    )
    .expect_output("not zero");
}

#[test]
fn test_body_cannot_claim_never() {
    // Adoption is bottom *elimination* only: `!` is introduced solely by
    // declared signatures, so a body that produces a real value cannot
    // check against a declared `!`.
    CliTest::new(
        r"
        fn lie(): ! {
            42
        }

        pub fn run(): Number {
            7
        }
        ",
    )
    .check()
    .expect_error("mismatch");
}

#[test]
fn test_resume_in_throw_arm_is_a_dedicated_error() {
    // `throw` returns `!`: nothing to resume — the perform site unwound.
    CliTest::new(
        r#"
        fn risky(): Number with Exception {
            Exception::throw!("boom")
        }

        pub fn run(): Number {
            with {
                Exception::throw(msg) => resume(1)
            } handle risky()
        }
        "#,
    )
    .check()
    .expect_error("cannot `resume` `Exception::throw`");
}

#[test]
fn test_resume_of_a_rethrow_is_still_an_error() {
    // Bottom elimination makes a `!` argument fit `resume`'s expected
    // type, so the never-arm check must fire before value unification.
    CliTest::new(
        r#"
        fn risky(): Number with Exception {
            Exception::throw!("boom")
        }

        pub fn run(): Number with Exception {
            with {
                Exception::throw(msg) => resume(Exception::throw!(msg))
            } handle risky()
        }
        "#,
    )
    .check()
    .expect_error("cannot `resume` `Exception::throw`");
}

#[test]
fn test_user_declared_never_method_may_be_abstract() {
    // Any method returning `!` may omit its default implementation, and
    // its performs unwind to the innermost handler exactly like `throw`.
    CliTest::new(
        r"
        unique(AB000000-0000-0000-0000-0000000000A1) ability Abort {
            fn abort(code: Number): !;
        }

        fn work(x: Number): Number with Abort {
            if (x > 10) {
                Abort::abort!(x)
            } else {
                x * 2
            }
        }

        pub fn run(): Number {
            let caught = with { Abort::abort(code) => code } handle work(50);
            let passed = with { Abort::abort(code) => code } handle work(3);
            caught + passed
        }
        ",
    )
    .expect_output("56");
}

#[test]
fn test_non_never_method_still_requires_a_body() {
    CliTest::new(
        r"
        unique(AB000000-0000-0000-0000-0000000000A2) ability Oracle {
            fn ask(q: String): Number;
        }

        pub fn run(): Number {
            7
        }
        ",
    )
    .check()
    .expect_error("needs a default implementation");
}

#[test]
fn test_resume_in_user_never_arm_is_rejected() {
    CliTest::new(
        r"
        unique(AB000000-0000-0000-0000-0000000000A3) ability Abort {
            fn abort(code: Number): !;
        }

        fn work(): Number with Abort {
            Abort::abort!(1)
        }

        pub fn run(): Number {
            with {
                Abort::abort(code) => resume(code)
            } handle work()
        }
        ",
    )
    .check()
    .expect_error("cannot `resume` `Abort::abort`");
}

#[test]
fn test_never_arm_can_rethrow() {
    // A catch arm is ordinary code: it can translate one never-typed
    // ability into another. The rethrow's `!` adopts the arm's required
    // result type, and at runtime both performs unwind.
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000A4) ability Abort {
            fn abort(code: Number): !;
        }

        fn work(): Number with Abort {
            Abort::abort!(5)
        }

        fn shield(): Number with Exception {
            with {
                Abort::abort(code) => Exception::throw!("rethrown")
            } handle work()
        }

        pub fn run(): Number {
            with {
                Exception::throw(msg) => 0 - 1
            } handle shield()
        }
        "#,
    )
    .expect_output("-1");
}

#[test]
fn test_alias_of_never_behaves_like_never() {
    // `type Bottom = !;` in an ability signature is exactly a spelled `!`:
    // the method may stay abstract, and its performs unwind.
    CliTest::new(
        r"
        type Bottom = !;

        unique(AB000000-0000-0000-0000-0000000000A7) ability Abort {
            fn abort(code: Number): Bottom;
        }

        fn work(x: Number): Number with Abort {
            if (x > 10) {
                Abort::abort!(x)
            }
            x * 2
        }

        pub fn run(): Number {
            let caught = with { Abort::abort(code) => code } handle work(50);
            let passed = with { Abort::abort(code) => code } handle work(3);
            caught + passed
        }
        ",
    )
    .expect_output("56");
}

#[test]
fn test_resume_on_alias_of_never_is_rejected() {
    CliTest::new(
        r"
        type Bottom = !;

        unique(AB000000-0000-0000-0000-0000000000A8) ability Abort {
            fn abort(code: Number): Bottom;
        }

        fn work(): Number with Abort {
            Abort::abort!(1)
        }

        pub fn run(): Number {
            with {
                Abort::abort(code) => resume(code)
            } handle work()
        }
        ",
    )
    .check()
    .expect_error("cannot `resume` `Abort::abort`");
}

#[test]
fn test_unhandled_abstract_never_method_is_a_runtime_fault() {
    // No handler and no default implementation: the perform is a fault,
    // like an uncaught exception.
    let output = CliTest::new(
        r"
        unique(AB000000-0000-0000-0000-0000000000A5) ability Abort {
            fn abort(code: Number): !;
        }

        pub fn run(): Number with Abort {
            Abort::abort!(9)
        }
        ",
    )
    .execute();
    assert!(!output.status.success(), "unhandled abort should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unhandled ability"),
        "expected an unhandled-ability fault, got: {stderr}"
    );
}

#[test]
fn test_never_perform_discards_the_delimited_computation() {
    // The unwind must drop everything between the perform and the handler
    // — including an inner (non-never) handler region — and the arm's
    // value lands at the handle expression's completion point.
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000A6) ability Ping {
            fn ping(): Number { 0 }
        }

        fn inner(): Number with Ping, Exception {
            let p = Ping::ping!();
            Exception::throw!("escape");
            p
        }

        pub fn run(): Number with Ping {
            let caught = with {
                Exception::throw(msg) => 1000
            } handle (with { Ping::ping() => resume(7) } handle inner());
            // The inner Ping handler must be fully uninstalled: a later
            // perform with no handler runs the default implementation.
            caught + Ping::ping!()
        }
        "#,
    )
    .expect_output("1000");
}

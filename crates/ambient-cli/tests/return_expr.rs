//! `return` expressions, end to end.
//!
//! `return expr` (and bare `return`, which returns unit) exits the
//! innermost function-like body — a fn item, a lambda, or a handler arm —
//! and its operand unifies with that body's return type. The expression
//! itself types as `!`, so it adopts any surrounding context (bottom
//! elimination), exactly like `Exception::throw!`.

mod common;

use common::CliTest;

#[test]
fn test_early_return_from_a_branch() {
    CliTest::new(
        r#"
        fn classify(value: Number): String {
            if (value < 0) {
                return "negative"
            }
            "non-negative"
        }

        pub fn run(): String {
            classify(-5)
        }
        "#,
    )
    .expect_output("negative");
}

#[test]
fn test_fallthrough_past_an_untaken_return() {
    CliTest::new(
        r#"
        fn classify(value: Number): String {
            if (value < 0) {
                return "negative"
            }
            "non-negative"
        }

        pub fn run(): String {
            classify(5)
        }
        "#,
    )
    .expect_output("non-negative");
}

#[test]
fn test_return_in_tail_position() {
    // A `return` as the body's final expression is redundant but legal:
    // the body's type is the (bottom-eliminated) `!`, which unifies with
    // the declared return type through the return target.
    CliTest::new(
        r#"
        fn double(value: Number): Number {
            return value * 2
        }

        pub fn run(): Number {
            double(21)
        }
        "#,
    )
    .expect_output("42");
}

#[test]
fn test_bare_return_returns_unit() {
    CliTest::new(
        r#"
        fn skip_positive(value: Number): () {
            if (value > 0) {
                return
            }
        }

        pub fn run(): Number {
            skip_positive(5);
            skip_positive(-5);
            42
        }
        "#,
    )
    .expect_output("42");
}

#[test]
fn test_return_in_match_arm() {
    CliTest::new(
        r#"
        fn describe(opt: Option<Number>): Number {
            match opt {
                Option::None => { return -1 }
                Option::Some(n) => n
            }
        }

        pub fn run(): Number {
            describe(Option::None) + describe(Option::Some(43))
        }
        "#,
    )
    .expect_output("42");
}

#[test]
fn test_return_exits_the_lambda_not_the_enclosing_fn() {
    // The lambda is its own return scope: `return n * 2` produces the
    // lambda's result, and the enclosing function keeps running.
    CliTest::new(
        r#"
        fn apply(f: (Number) -> Number, x: Number): Number {
            f(x)
        }

        pub fn run(): Number {
            let doubled = apply((n) => { return n * 2 }, 20);
            doubled + 2
        }
        "#,
    )
    .expect_output("42");
}

#[test]
fn test_return_value_pins_an_unannotated_return_type() {
    // Early returns flow into the scheme's return variable, so callers
    // see the real type even without an annotation.
    CliTest::new(
        r#"
        fn pick(flag: Bool) {
            if (flag) {
                return 40
            }
            2
        }

        pub fn run(): Number {
            pick(true) + pick(false)
        }
        "#,
    )
    .expect_output("42");
}

#[test]
fn test_return_type_mismatch_is_an_error() {
    CliTest::new(
        r#"
        fn confused(value: Number): Number {
            if (value < 0) {
                return "negative"
            }
            value
        }

        pub fn run(): Number {
            confused(1)
        }
        "#,
    )
    .check()
    .expect_error("type mismatch: expected `Number`, found `String`");
}

#[test]
fn test_conflicting_returns_are_an_error() {
    // Both returns feed the same return target, so they must agree even
    // when the return type is unannotated.
    CliTest::new(
        r#"
        fn confused(flag: Bool) {
            if (flag) {
                return 1
            }
            "two"
        }

        pub fn run(): Number {
            confused(true)
        }
        "#,
    )
    .check()
    .expect_failure();
}

#[test]
fn test_return_outside_a_function_is_an_error() {
    CliTest::new(
        r#"
        const ANSWER = return 42;

        pub fn run(): Number {
            ANSWER
        }
        "#,
    )
    .check()
    .expect_error("`return` is only meaningful inside a function body");
}

#[test]
fn test_return_in_handler_arm_completes_the_arm() {
    // An arm is its own frame and its own return scope: `return -1`
    // completes the arm, and the arm's value becomes the handle
    // expression's result.
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000B1) ability Fail {
            fn fail(msg: String): !;
        }

        fn risky(value: Number): Number with Fail {
            if (value < 0) {
                Fail::fail!("negative")
            }
            value
        }

        pub fn run(): Number {
            with {
                Fail::fail(msg) => { return -1 }
            } handle risky(-5)
        }
        "#,
    )
    .expect_output("-1");
}

//! Handler values, sandbox clauses, and the platform ability namespace policy.

mod common;
use common::*;

// ─────────────────────────────────────────────────────────────────────────────
// Handler Value Tests (Milestone 13)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_handler_value_basic() {
    CliTest::new(
        r#"
        fn simple_function(): Number { 100 }

        fn test_handler_value(): Number {
            let mock_console = {
                core::system::Stdio::out(msg) => resume(())
            };
            with mock_console handle simple_function()
        }

        fn run(): Number { test_handler_value() }
    "#,
    )
    .expect_output("100");
}

#[test]
fn test_handler_value_multiple() {
    CliTest::new(
        r#"
        fn simple_function(): Number { 200 }

        fn test_multiple_handlers(): Number {
            let handler1 = { core::system::Stdio::out(msg) => resume(()) };
            let handler2 = { Exception::throw(err) => resume(()) };
            with handler1, handler2 handle simple_function()
        }

        fn run(): Number { test_multiple_handlers() }
    "#,
    )
    .expect_output("200");
}

#[test]
fn test_handler_value_with_inline() {
    CliTest::new(
        r#"
        fn simple_function(): Number { 300 }

        fn test_mixed(): Number {
            let mock_console = { core::system::Stdio::out(msg) => resume(()) };
            with mock_console, {
                Exception::throw(err) => {
                    resume(())
                }
            } handle simple_function()
        }

        fn run(): Number { test_mixed() }
    "#,
    )
    .expect_output("300");
}

/// Composing a handler value with an inline override for the *same* ability
/// installs left-to-right, so the later handler wins ("last wins"). Here the
/// inline `resume(2)` shadows the value's `resume(1)`.
#[test]
fn test_handler_value_override_last_wins() {
    CliTest::new(
        r#"
        ability Choice {
          fn pick(): Number;
        }

        fn body(): Number with Choice {
          Choice::pick!()
        }

        fn test(): Number {
          let first = { Choice::pick() => resume(1) };
          with first, { Choice::pick() => resume(2) } handle body()
        }

        fn run(): Number { test() }
    "#,
    )
    .expect_output("2");
}

/// A handler *value* (let-bound, so it installs through the `HandlerValue`
/// path) may capture variables from its enclosing scope. The arm body reads
/// `offset`, a local of the surrounding function, proving the captured
/// environment reaches the method function at runtime.
#[test]
fn test_handler_value_captures_outer_variable() {
    CliTest::new(
        r#"
        ability Choice {
          fn pick(): Number;
        }

        fn body(): Number with Choice {
          Choice::pick!()
        }

        fn test(): Number {
          let offset = 40;
          let handler = { Choice::pick() => resume(offset + 2) };
          with handler handle body()
        }

        fn run(): Number { test() }
    "#,
    )
    .expect_output("42");
}

/// Two arms of one handler value capture *different* outer variables. They
/// share one runtime capture array, so each name must land on its own
/// stable slot; whichever arm fires reads the right value.
#[test]
fn test_handler_value_multi_arm_captures() {
    CliTest::new(
        r#"
        ability Pair {
          fn left(): Number;
          fn right(): Number;
        }

        fn body(): Number with Pair {
          Pair::left!() + Pair::right!()
        }

        fn test(): Number {
          let a = 10;
          let b = 20;
          let handler = {
            Pair::left() => resume(a),
            Pair::right() => resume(b)
          };
          with handler handle body()
        }

        fn run(): Number { test() }
    "#,
    )
    .expect_output("30");
}

/// An inline multi-method brace for one ability installs a single
/// method-dispatched `HandlerValue`, so each perform reaches its own arm.
/// `left` yields 1 and `right` yields 2, giving `1*10 + 2`; a
/// method-agnostic install (the old per-arm `Handle` path) would run one
/// arm for both performs.
#[test]
fn test_inline_multi_method_dispatch() {
    CliTest::new(
        r#"
        ability Pair {
          fn left(): Number;
          fn right(): Number;
        }

        fn body(): Number with Pair {
          Pair::left!() * 10 + Pair::right!()
        }

        fn run(): Number {
          with { Pair::left() => resume(1), Pair::right() => resume(2) } handle body()
        }
    "#,
    )
    .expect_output("12");
}

#[test]
fn test_example_handler_value() {
    let output = ambient_cmd()
        .arg("run")
        .arg("../../examples/handler_value_test")
        .output()
        .expect("failed to execute command");

    assert!(
        output.status.success(),
        "handler_value_test package should run successfully: {:?}\nstderr: {}",
        output,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("100"), "expected 100 in output: {stdout}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Sandbox Tests (Milestone 14)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_sandbox_pure_computation() {
    CliTest::new(
        r#"
        fn pure_add(x: Number, y: Number): Number {
            x + y
        }

        fn run(): Number {
            sandbox {
                pure_add(2, 3)
            }
        }
    "#,
    )
    .expect_output("5");
}

#[test]
fn test_sandbox_with_allowed_ability() {
    CliTest::new(
        r#"
        fn compute(): Number {
            42
        }

        fn run(): Number {
            sandbox with core::system::Stdio {
                compute()
            }
        }
    "#,
    )
    .expect_output("42");
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform ability namespace policy: platform abilities must be written
// `core::system::X` in every position (with clauses, effect rows, handler
// arms, sandbox clauses, performs); user abilities and Exception stay
// bare.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_with_clause_requires_platform_namespace() {
    CliTest::new(
        r#"
        pub fn run(): () with Stdio {
            core::system::Stdio::out!("hi")
        }
        "#,
    )
    .expect_error("qualify it as `core::system::");
}

#[test]
fn test_handler_arm_requires_platform_namespace() {
    CliTest::new(
        r#"
        fn speak(): () with core::system::Stdio {
            core::system::Stdio::out!("hi")
        }

        pub fn run(): () {
            with {
                Stdio::out(msg) => {
                    resume(())
                }
            } handle speak()
        }
        "#,
    )
    .expect_error("qualify it as `core::system::");
}

#[test]
fn test_sandbox_clause_requires_platform_namespace() {
    CliTest::new(
        r#"
        pub fn run(): Number {
            sandbox with Stdio {
                42
            }
        }
        "#,
    )
    .expect_error("qualify it as `core::system::");
}

#[test]
fn test_effect_row_annotation_requires_platform_namespace() {
    CliTest::new(
        r#"
        fn call(f: () -> () with Log): () with core::system::Log {
            f()
        }

        pub fn run(): () {
            ()
        }
        "#,
    )
    .expect_error("qualify it as `core::system::");
}

#[test]
fn test_exception_may_not_be_namespaced() {
    // Exception is a language builtin, never platform-qualified.
    CliTest::new(
        r#"
        pub fn run(): () with core::system::Exception {
            ()
        }
        "#,
    )
    .expect_error("unknown ability");
}

#[test]
fn test_local_ability_shadows_platform_name() {
    // A local declaration reclaims the bare name in every position; the
    // platform ability stays reachable through its prefix.
    CliTest::new(
        r#"
        ability Stdio {
            fn shout(message: String): String;
        }

        fn noise(): String with Stdio {
            Stdio::shout!("quiet")
        }

        pub fn run(): () with core::system::Stdio {
            let loud = with {
                Stdio::shout(msg) => {
                    resume(msg.concat("!"))
                }
            } handle noise();
            core::system::Stdio::out!(loud)
        }
        "#,
    )
    .expect_output("quiet!");
}

#[test]
fn test_time_wait_accepts_duration() {
    // `core::system::Time::wait` takes a `core::time::Duration`. This exercises
    // the whole path: the ability signature references a core nominal type,
    // the checker unifies the caller's `Duration` against the signature's
    // (unexpanded) reference, and the host handler decodes the record and
    // sleeps. A successful numeric result proves compile *and* dispatch.
    CliTest::new(
        r#"
        use core::time::Duration;

        pub fn run(): Number with core::system::Time {
            core::system::Time::wait!(Duration::from_millis(1));
            7
        }
        "#,
    )
    .expect_output("7");
}

#[test]
fn test_time_wait_rejects_raw_number() {
    // A bare millisecond count is no longer accepted: the Named/Nominal
    // bridge only unifies the signature's `Duration` against a real
    // `Duration`, not against `number`.
    CliTest::new(
        r#"
        pub fn run(): () with core::system::Time {
            core::system::Time::wait!(20);
        }
        "#,
    )
    .expect_error("type mismatch");
}

#[test]
fn test_sandbox_nested_pure() {
    CliTest::new(
        r#"
        fn factorial(n: Number): Number {
            if n <= 1 { 1 } else { n * factorial(n - 1) }
        }

        fn run(): Number {
            sandbox {
                factorial(5)
            }
        }
    "#,
    )
    .expect_output("120");
}

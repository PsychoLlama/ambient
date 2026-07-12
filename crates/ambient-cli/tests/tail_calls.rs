//! Proper tail calls end-to-end.
//!
//! The compiler emits `TailCall` / `TailCallClosure` (frame reuse) for every
//! call in tail position, so tail recursion runs in constant call-stack space
//! — a chain far past the VM's `max_call_depth` (1000) returns a value instead
//! of overflowing. Each test drives a program deep enough that a non-tail
//! version would overflow, and asserts the correct result.
//!
//! The one thing tail calls do *not* buy: a handler that `resume`s on every
//! iteration still accumulates one continuation frame per cycle (`resume` is
//! not a tail position in this phase), so a deep *actively-handled* effect
//! loop still overflows at 1000. That limit is a property of the effect
//! system, orthogonal to tail calls; see `deep_effect_loop_runs_default_impl`
//! and `handled_effect_loop_has_correct_semantics`.

mod common;
use common::*;

/// Deliberately far past `max_call_depth` (1000): a non-tail recursion this
/// deep overflows the call stack.
const DEEP: u32 = 100_000;

#[test]
fn deep_self_tail_recursion() {
    // `count_down(100000, 0)` counts down in tail position; before tail calls
    // this overflowed the call stack.
    CliTest::new(format!(
        r#"
        fn count_down(n: Number, acc: Number): Number {{
            if n <= 0 {{
                acc
            }} else {{
                count_down(n - 1, acc + 1)
            }}
        }}

        pub fn run(): Number {{
            count_down({DEEP}, 0)
        }}
    "#
    ))
    .expect_output("100000");
}

#[test]
fn deep_mutual_tail_recursion() {
    // `is_even`/`is_odd` tail-call each other; 100001 is odd, so `is_even`
    // returns false. Also exercises SCC group hashing with tail calls (the
    // two functions form one recursive group).
    CliTest::new(format!(
        r#"
        fn is_even(n: Number): Bool {{
            if n == 0 {{ true }} else {{ is_odd(n - 1) }}
        }}

        fn is_odd(n: Number): Bool {{
            if n == 0 {{ false }} else {{ is_even(n - 1) }}
        }}

        pub fn run(): Bool {{
            is_even({})
        }}
    "#,
        DEEP + 1
    ))
    .expect_output("false");
}

#[test]
fn deep_tail_call_through_function_value() {
    // The callee is a local bound to the function value, so the recursive
    // call compiles to `TailCallClosure` (the closure/function-value path)
    // rather than a direct `TailCall`. It still recurses in constant space.
    CliTest::new(format!(
        r#"
        fn spin(n: Number, acc: Number): Number {{
            let f = spin;
            if n <= 0 {{
                acc
            }} else {{
                f(n - 1, acc + 1)
            }}
        }}

        pub fn run(): Number {{
            spin({DEEP}, 0)
        }}
    "#
    ))
    .expect_output("100000");
}

#[test]
fn deep_trait_bounded_tail_recursion() {
    // A `T: Eq`-bounded generic that tail-recurses, forwarding its hidden
    // dictionary parameter through every iteration (the `DictSource::Param`
    // path). `item.eq(target)` dispatches through the dictionary each step.
    CliTest::new(format!(
        r#"
        unique(A1B2C3D4-0000-0000-0000-0000000000C1) struct Money {{ cents: Number }}

        impl Eq for Money {{
            fn eq(self, other: Money): Bool {{
                self.cents == other.cents
            }}
        }}

        fn count_eq<T: Eq>(target: T, item: T, n: Number, acc: Number): Number {{
            if n <= 0 {{
                acc
            }} else {{
                let bump = if item.eq(target) {{ 1 }} else {{ 0 }};
                count_eq(target, item, n - 1, acc + bump)
            }}
        }}

        pub fn run(): Number {{
            count_eq(Money {{ cents: 5 }}, Money {{ cents: 5 }}, {DEEP}, 0)
        }}
    "#
    ))
    .expect_output("100000");
}

#[test]
fn non_tail_recursion_still_overflows() {
    // `1 + deep(n - 1)` wraps the recursive call in an addition, so it is NOT
    // in tail position: it still pushes a frame per call and overflows at
    // `max_call_depth`.
    CliTest::new(format!(
        r#"
        fn deep(n: Number): Number {{
            if n <= 0 {{
                0
            }} else {{
                1 + deep(n - 1)
            }}
        }}

        pub fn run(): Number {{
            deep({DEEP})
        }}
    "#
    ))
    .expect_error("call stack overflow");
}

#[test]
fn deep_effect_loop_runs_default_impl() {
    // A tail-recursive function that performs an ability every iteration.
    // With no handler in scope, each perform runs the method's default
    // implementation (a plain call that returns immediately), so the only
    // thing that could accumulate frames is the recursion itself — and it
    // does not. Proves tail calls compose with performs.
    CliTest::new(format!(
        r#"
        unique(AB000000-0000-0000-0000-0000000000E1) ability Tick {{
            fn tick(): Number {{ 0 }}
        }}

        fn loop_perform(n: Number, acc: Number): Number with Tick {{
            if n <= 0 {{
                acc
            }} else {{
                let x = Tick::tick!();
                loop_perform(n - 1, acc + x)
            }}
        }}

        pub fn run(): Number with Tick {{
            loop_perform({DEEP}, 0)
        }}
    "#
    ))
    .expect_output("0");
}

#[test]
fn handled_effect_loop_has_correct_semantics() {
    // The same loop under a resuming handler. The tail recursion is
    // constant-space, but each `resume` (not a tail position in this phase)
    // adds one continuation frame per cycle, so this depth must stay under
    // `max_call_depth`. At 500 iterations, each `resume(1)` contributes 1 to
    // the accumulator: the result is 500, proving handler semantics are
    // correct. (A deeper actively-handled loop overflows on `resume`, not on
    // the recursion — orthogonal to tail calls.)
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000E2) ability Tick {
            fn tick(): Number { 0 }
        }

        fn loop_perform(n: Number, acc: Number): Number with Tick {
            if n <= 0 {
                acc
            } else {
                let x = Tick::tick!();
                loop_perform(n - 1, acc + x)
            }
        }

        pub fn run(): Number {
            with { Tick::tick() => resume(1) } handle loop_perform(500, 0)
        }
    "#,
    )
    .expect_output("500");
}

#[test]
fn core_lib_fold_map_filter_over_long_list() {
    // `List` `fold`/`map`/`filter` delegate to accumulator-style helpers
    // (`fold_from`/`map_from`/`filter_from`) whose recursive calls are in
    // tail position, so they now traverse a list longer than 1000 elements.
    // The list itself is built by a tail-recursive helper (a literal that
    // long is impractical, and `List::range` recurses non-tail through
    // `concat`, so it can't build one either). Every element is 1:
    //   fold sum        = 3000
    //   map length      = 3000
    //   filter(==1) len = 3000
    // => 9000.
    CliTest::new(
        r#"
        fn build(n: Number, acc: List<Number>): List<Number> {
            if n <= 0 { acc } else { build(n - 1, acc.append(1)) }
        }

        pub fn run(): Number {
            let big = build(3000, []);
            let total = big.fold(0, (acc: Number, x: Number) => acc + x);
            let mapped = big.map((x: Number) => x + 1);
            let kept = big.filter((x: Number) => x == 1);
            total + mapped.length() + kept.length()
        }
    "#,
    )
    .expect_output("9000");
}

#[test]
fn fibonacci_example_tail_variant() {
    // The `examples/fibonacci` `fib_tail`/`fib_tail_helper` pair is genuine
    // tail recursion; its output must be unchanged. `fib_tail(20)` is the
    // 20th Fibonacci number, 6765.
    CliTest::new(
        r#"
        fn fib_tail(n: Number): Number {
            fib_tail_helper(n, 0, 1)
        }

        fn fib_tail_helper(n: Number, a: Number, b: Number): Number {
            if n == 0 {
                a
            } else {
                fib_tail_helper(n - 1, b, a + b)
            }
        }

        pub fn run(): Number {
            fib_tail(20)
        }
    "#,
    )
    .expect_output("6765");
}

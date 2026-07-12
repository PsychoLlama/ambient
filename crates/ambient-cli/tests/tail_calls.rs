//! Proper tail calls end-to-end.
//!
//! The compiler emits `TailCall` / `TailCallClosure` (frame reuse) for every
//! call in tail position, so tail recursion runs in constant call-stack space
//! — a chain far past the VM's `max_call_depth` (1000) returns a value instead
//! of overflowing. Each test drives a program deep enough that a non-tail
//! version would overflow, and asserts the correct result.
//!
//! Handler-driven effect loops are constant-space too: a tail-position
//! `resume` compiles to `TailResume`, which discards the arm frame before
//! reinstating the continuation, so a handler that `resume`s on every cycle
//! no longer parks a frame per cycle — see
//! `handled_effect_loop_is_constant_space` and `tail_perform_and_tail_resume`.
//! A *non-tail* `resume` (e.g. `let x = resume(v); ...`) still keeps the
//! parking `Resume`, by design; see `non_tail_resume_still_works`.

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
fn handled_effect_loop_is_constant_space() {
    // The same loop under a resuming handler. The arm's `resume(1)` sits in
    // tail position, so it compiles to `TailResume`: the arm frame is
    // discarded before the continuation is reinstated, and the loop runs in
    // constant frame space. At 100_000 iterations — far past `max_call_depth`
    // (1000), which the parking `Resume` would have overflowed — each
    // `resume(1)` contributes 1 to the accumulator, so the result is 100_000.
    CliTest::new(format!(
        r#"
        unique(AB000000-0000-0000-0000-0000000000E2) ability Tick {{
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

        pub fn run(): Number {{
            with {{ Tick::tick() => resume(1) }} handle loop_perform({DEEP}, 0)
        }}
    "#
    ))
    .expect_output("100000");
}

#[test]
fn tail_resume_under_if_and_match_branches_is_constant_space() {
    // The arm's `resume` sits inside `if`/`match` branches rather than being
    // the whole arm body. `if`/`match` propagate the tail flag, so every
    // branch's `resume` is still a tail position and compiles to
    // `TailResume` — the loop stays constant-space at 100_000 cycles. The
    // `tick` argument alternates the branch taken; both branches resume with
    // 1, so the accumulator still reaches 100_000.
    CliTest::new(format!(
        r#"
        unique(AB000000-0000-0000-0000-0000000000E5) ability Tick {{
            fn tick(flag: Bool): Number {{ 0 }}
        }}

        fn loop_perform(n: Number, acc: Number): Number with Tick {{
            if n <= 0 {{
                acc
            }} else {{
                let x = Tick::tick!(n % 2 == 0);
                loop_perform(n - 1, acc + x)
            }}
        }}

        pub fn run(): Number {{
            with {{
                Tick::tick(flag) => if flag {{ resume(1) }} else {{
                    match flag {{
                        true => resume(1),
                        false => resume(1),
                    }}
                }}
            }} handle loop_perform({DEEP}, 0)
        }}
    "#
    ))
    .expect_output("100000");
}

#[test]
fn tail_perform_and_tail_resume() {
    // The canonical constant-space effect loop: a tail-recursive performing
    // function driven by a tail-resuming handler, both running in constant
    // frame space. Neither the recursion (proper tail calls) nor the handler
    // (tail resume) grows the stack, so 100_000 iterations complete. The
    // handler resumes with the running iteration count doubled back down —
    // here just `resume(2)` — so the accumulator reaches 200_000.
    CliTest::new(format!(
        r#"
        unique(AB000000-0000-0000-0000-0000000000E6) ability Step {{
            fn step(): Number {{ 0 }}
        }}

        fn drive(n: Number, acc: Number): Number with Step {{
            if n <= 0 {{
                acc
            }} else {{
                let delta = Step::step!();
                drive(n - 1, acc + delta)
            }}
        }}

        pub fn run(): Number {{
            with {{ Step::step() => resume(2) }} handle drive({DEEP}, 0)
        }}
    "#
    ))
    .expect_output("200000");
}

#[test]
fn non_tail_resume_still_works() {
    // A `resume` that is *not* in tail position — its result is bound and
    // then used — keeps the parking `Resume` opcode and its per-cycle frame
    // cost. It stays semantically correct at shallow depth: each cycle
    // resumes with `resume(1) + 0`, so a 3-iteration loop accumulates 3.
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000E7) ability Tick {
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
            with { Tick::tick() => { let x = resume(1); x + 0 } } handle loop_perform(3, 0)
        }
    "#,
    )
    .expect_output("3");
}

#[test]
fn core_lib_fold_map_filter_over_long_list() {
    // `List` `fold`/`map`/`filter` delegate to accumulator-style helpers
    // (`fold_from`/`map_from`/`filter_from`) whose recursive calls are in
    // tail position, so they now traverse a list longer than 1000 elements.
    // The list itself is built by a tail-recursive helper (a literal that
    // long is impractical; `List::range` would work too, but a hand-rolled
    // builder keeps this test independent of the core-lib helper). Every
    // element is 1:
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
fn core_lib_any_all_over_long_list() {
    // `any`/`all` delegate to `any_from`/`all_from`, whose recursion is the
    // right operand of `||`/`&&`. Short-circuit right operands are now tail
    // positions, so both traverse a list longer than 1000 elements. The list
    // is all 1s except the last element (2), exercising both the hit and miss
    // paths that scan the whole list:
    //   any(== 2)  finds the final element  => true   (scans all 3000)
    //   any(== 9)  never matches            => false  (scans all 3000)
    //   all(== 1)  fails on the final 2     => false  (scans all 3000)
    //   all(>= 1)  holds everywhere         => true   (scans all 3000)
    // Encoded as 1 + 0 + 0 + 1 = 2 booleans-as-numbers.
    CliTest::new(
        r#"
        fn build(n: Number, acc: List<Number>): List<Number> {
            if n <= 1 { acc.append(2) } else { build(n - 1, acc.append(1)) }
        }

        fn as_num(b: Bool): Number { if b { 1 } else { 0 } }

        pub fn run(): Number {
            let big = build(3000, []);
            as_num(big.any((x: Number) => x == 2))
                + as_num(big.any((x: Number) => x == 9))
                + as_num(big.all((x: Number) => x == 1))
                + as_num(big.all((x: Number) => x >= 1))
        }
    "#,
    )
    .expect_output("2");
}

#[test]
fn core_lib_range_over_long_span() {
    // `List::range` now accumulates front-to-back via `range_from`, whose
    // recursive call is in tail position (no `concat` wrapping it), so it
    // builds spans longer than 1000. `range(0, 3000)` sums to 3000*2999/2.
    CliTest::new(
        r#"
        pub fn run(): Number {
            List::range(0, 3000).fold(0, (acc: Number, x: Number) => acc + x)
        }
    "#,
    )
    .expect_output("4498500");
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

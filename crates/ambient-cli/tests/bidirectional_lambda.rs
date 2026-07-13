//! Bidirectional checking of lambda arguments: an unannotated lambda passed
//! to a call adopts its parameter types from the callee's expected function
//! type, so a bound method (`x.eq(item)` where `x: T` and `T: Eq`) resolves
//! without a written `(x: T)` annotation.

mod common;
use common::*;

#[test]
fn test_unannotated_bound_method_lambda() {
    // `x` is unannotated, yet `x.eq(item)` type-checks: `any` expects a
    // `(T) -> Bool` predicate, and the checker seeds `x: T` from it before
    // checking the body, so the `Eq` bound resolves through the dictionary.
    CliTest::new(
        r#"
        fn contains<T: Eq>(items: List<T>, item: T): Bool {
            items.any((x) => x.eq(item))
        }

        pub fn run(): Bool with core::system::Stdio {
            core::system::Stdio::out!(if contains([1, 2, 3], 2) { "yes" } else { "no" });
            contains([1, 2, 3], 9)
        }
    "#,
    )
    .expect_output("yes");
}

#[test]
fn test_nested_unannotated_bound_method_lambdas() {
    // The seed threads through nested lambdas: the outer `row` and the inner
    // `x` are both unannotated, and `x.eq(item)` still resolves the `Eq`
    // bound on `T`.
    CliTest::new(
        r#"
        fn grid_contains<T: Eq>(rows: List<List<T>>, item: T): Bool {
            rows.any((row) => row.any((x) => x.eq(item)))
        }

        pub fn run(): Bool with core::system::Stdio {
            core::system::Stdio::out!(
                if grid_contains([[1], [2, 3]], 3) { "found" } else { "missing" }
            );
            grid_contains([[1], [2, 3]], 9)
        }
    "#,
    )
    .expect_output("found");
}

#[test]
fn test_ordinary_call_seeds_lambda_params() {
    // The same bidirectional seeding applies to ordinary (non-method) calls:
    // `apply`'s `f` parameter is `(Number) -> Number`, so `n` needs no
    // annotation.
    CliTest::new(
        r#"
        fn apply(f: (Number) -> Number, x: Number): Number {
            f(x)
        }

        pub fn run(): Number with core::system::Stdio {
            core::system::Stdio::out!(if apply((n) => n + 1, 41) == 42 { "ok" } else { "bad" });
            0
        }
    "#,
    )
    .expect_output("ok");
}

#[test]
fn test_explicit_annotation_still_wins_and_conflicts_error() {
    // An explicit annotation is never overwritten by the expected type: a
    // `Bool` annotation on a lambda passed where a `Number` predicate is
    // expected still fails, at the argument.
    CliTest::new(
        r#"
        fn use_sum(items: List<Number>): Number {
            items.fold(0, (acc, x: Bool) => acc)
        }
    "#,
    )
    .check()
    .expect_error("type mismatch");
}

#[test]
fn test_lambda_arity_mismatch_reports_not_panics() {
    // A lambda whose arity differs from the expected function type does not
    // get seeded (nothing to seed against); the ordinary function-type
    // mismatch is reported rather than panicking on an out-of-bounds seed.
    CliTest::new(
        r#"
        fn use_any(items: List<Number>): Bool {
            items.any((x, y) => true)
        }
    "#,
    )
    .check()
    .expect_error("type mismatch");
}

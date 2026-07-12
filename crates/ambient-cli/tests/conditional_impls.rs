//! Conditional (generic) trait impls as dictionary sources:
//! `impl<T: Eq> Eq for Pair<T>`. Solving `Pair<Money>: Eq` unifies the impl's
//! target against the concrete type, recursively solves the impl's own bounds
//! (`Money: Eq`), and builds a dictionary whose slots are closures over the
//! inner dictionaries. Exercised end-to-end through a bounded generic.

mod common;
use common::*;

// `Money: Eq` plus a generic `Pair<T>` with a conditional `Eq` impl, and a
// bounded generic `same` that dispatches `eq` through a dictionary.
const PRELUDE: &str = r#"
    unique(C0000000-0000-0000-0000-0000000000E1) struct Money { cents: Number }

    impl Eq for Money {
        fn eq(self, other: Money): Bool {
            self.cents == other.cents
        }
    }

    unique(C0000000-0000-0000-0000-0000000000E2) struct Pair<T> { first: T, second: T }

    impl<T: Eq> Eq for Pair<T> {
        fn eq(self, other: Pair<T>): Bool {
            self.first.eq(other.first) && self.second.eq(other.second)
        }
    }

    fn same<T: Eq>(a: T, b: T): Bool {
        a.eq(b)
    }
"#;

#[test]
fn conditional_impl_satisfies_bound_through_bounded_generic() {
    // `same(Pair<Money>, Pair<Money>)` solves `Pair<Money>: Eq` through the
    // conditional impl: the built dictionary's `eq` slot is a closure over
    // `Money`'s Eq dictionary.
    CliTest::new(format!(
        r#"
        {PRELUDE}
        fn run(): Bool {{
            same(
                Pair {{ first: Money {{ cents: 1 }}, second: Money {{ cents: 2 }} }},
                Pair {{ first: Money {{ cents: 1 }}, second: Money {{ cents: 2 }} }}
            )
        }}
    "#
    ))
    .expect_output("true");
}

#[test]
fn conditional_impl_reports_inequality() {
    // The same path, but the pairs differ in their second field: the inner
    // `Money::eq` fires and the `&&` short-circuits to false.
    CliTest::new(format!(
        r#"
        {PRELUDE}
        fn run(): Bool {{
            same(
                Pair {{ first: Money {{ cents: 1 }}, second: Money {{ cents: 2 }} }},
                Pair {{ first: Money {{ cents: 1 }}, second: Money {{ cents: 9 }} }}
            )
        }}
    "#
    ))
    .expect_output("false");
}

#[test]
fn conditional_impl_dictionary_through_bounded_value() {
    // A bounded generic used as a *value* whose instantiation is
    // `Pair<Money>` captures a `DictSource::Generic` dictionary: the
    // synthesized forwarding closure carries the conditional impl's
    // closure-tuple dictionary into `same`.
    CliTest::new(format!(
        r#"
        {PRELUDE}
        fn apply_pair<A>(f: (A, A) -> Bool, x: A, y: A): Bool {{
            f(x, y)
        }}

        fn run(): Bool {{
            apply_pair(
                same,
                Pair {{ first: Money {{ cents: 1 }}, second: Money {{ cents: 2 }} }},
                Pair {{ first: Money {{ cents: 1 }}, second: Money {{ cents: 2 }} }}
            )
        }}
    "#
    ))
    .expect_output("true");
}

#[test]
fn conditional_impl_two_levels_deep() {
    // `Pair<Pair<Money>>: Eq` recurses: the outer dictionary's slot closes
    // over the inner `Pair<Money>: Eq` dictionary, which itself closes over
    // `Money`'s Eq dictionary.
    CliTest::new(format!(
        r#"
        {PRELUDE}
        fn run(): Bool {{
            let a = Pair {{
                first: Pair {{ first: Money {{ cents: 1 }}, second: Money {{ cents: 2 }} }},
                second: Pair {{ first: Money {{ cents: 3 }}, second: Money {{ cents: 4 }} }}
            }};
            let b = Pair {{
                first: Pair {{ first: Money {{ cents: 1 }}, second: Money {{ cents: 2 }} }},
                second: Pair {{ first: Money {{ cents: 3 }}, second: Money {{ cents: 4 }} }}
            }};
            same(a, b)
        }}
    "#
    ))
    .expect_output("true");
}

#[test]
fn conditional_impl_two_levels_deep_reports_inequality() {
    CliTest::new(format!(
        r#"
        {PRELUDE}
        fn run(): Bool {{
            let a = Pair {{
                first: Pair {{ first: Money {{ cents: 1 }}, second: Money {{ cents: 2 }} }},
                second: Pair {{ first: Money {{ cents: 3 }}, second: Money {{ cents: 4 }} }}
            }};
            let b = Pair {{
                first: Pair {{ first: Money {{ cents: 1 }}, second: Money {{ cents: 2 }} }},
                second: Pair {{ first: Money {{ cents: 3 }}, second: Money {{ cents: 99 }} }}
            }};
            same(a, b)
        }}
    "#
    ))
    .expect_output("false");
}

#[test]
fn direct_method_call_on_conditional_impl() {
    // A direct `pair.eq(other)` on a concrete `Pair<Money>` (no bounded
    // generic in between). Whether the checker threads the impl's hidden
    // dictionary here is what this pins.
    CliTest::new(format!(
        r#"
        {PRELUDE}
        fn run(): Bool {{
            let a = Pair {{ first: Money {{ cents: 5 }}, second: Money {{ cents: 6 }} }};
            let b = Pair {{ first: Money {{ cents: 5 }}, second: Money {{ cents: 6 }} }};
            a.eq(b)
        }}
    "#
    ))
    .expect_output("true");
}

#[test]
fn conditional_impl_operator_sugar() {
    // `==` on a `Pair<Money>` value desugars to the conditional impl's `eq`.
    CliTest::new(format!(
        r#"
        {PRELUDE}
        fn run(): Bool {{
            let a = Pair {{ first: Money {{ cents: 5 }}, second: Money {{ cents: 6 }} }};
            let b = Pair {{ first: Money {{ cents: 5 }}, second: Money {{ cents: 7 }} }};
            a == b
        }}
    "#
    ))
    .expect_output("false");
}

#[test]
fn conditional_impl_operator_equal() {
    // `==` on two equal `Pair<Money>` values dispatches through the
    // conditional impl and reports equality.
    CliTest::new(format!(
        r#"
        {PRELUDE}
        fn run(): Bool {{
            let a = Pair {{ first: Money {{ cents: 5 }}, second: Money {{ cents: 6 }} }};
            let b = Pair {{ first: Money {{ cents: 5 }}, second: Money {{ cents: 6 }} }};
            a == b
        }}
    "#
    ))
    .expect_output("true");
}

#[test]
fn unsatisfied_inner_bound_is_rejected() {
    // `Pair<Plain>: Eq` needs `Plain: Eq`, which doesn't exist — the
    // conditional impl's inner bound is unsatisfied, so the call is rejected.
    CliTest::new(format!(
        r#"
        {PRELUDE}
        unique(C0000000-0000-0000-0000-0000000000E3) struct Plain {{ n: Number }}

        fn run(): Bool {{
            same(
                Pair {{ first: Plain {{ n: 1 }}, second: Plain {{ n: 2 }} }},
                Pair {{ first: Plain {{ n: 1 }}, second: Plain {{ n: 2 }} }}
            )
        }}
    "#
    ))
    .check()
    .expect_failure();
}

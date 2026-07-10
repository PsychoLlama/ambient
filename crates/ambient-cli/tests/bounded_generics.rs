//! Trait bounds on generics (`fn f<T: Eq>(...)`): dictionary-passing
//! dispatch end-to-end — bound-method calls in generic bodies, operator
//! sugar on bounded parameters, concrete-impl dictionaries at call sites,
//! dictionary forwarding between bounded generics, and the error paths.

mod common;
use common::*;

// A `Money` type implementing the prelude's Eq and Ord, used across tests.
const MONEY: &str = r#"
    unique(A1B2C3D4-0000-0000-0000-00000000BB01) struct Money { cents: Number }

    impl Eq for Money {
        fn eq(self, other: Money): Bool {
            self.cents == other.cents
        }
    }

    impl Ord for Money {
        fn cmp(self, other: Money): Number {
            if self.cents < other.cents { 0 - 1 } else {
                if self.cents > other.cents { 1 } else { 0 }
            }
        }
    }
"#;

#[test]
fn bound_method_call_in_generic_body() {
    // `x.eq(y)` inside a generic body dispatches through the Eq
    // dictionary; the call site supplies Money's impl.
    CliTest::new(format!(
        r#"
        {MONEY}
        fn same<T: Eq>(a: T, b: T): Bool {{
            a.eq(b)
        }}

        fn run(): Bool {{
            same(Money {{ cents: 100 }}, Money {{ cents: 100 }})
        }}
    "#
    ))
    .expect_output("true");
}

#[test]
fn operator_on_bounded_param() {
    // `==` and `<` on a bounded parameter dispatch through the reserved
    // operator traits' dictionaries, including the Ord result adaptation.
    CliTest::new(format!(
        r#"
        {MONEY}
        fn min_of<T: Ord>(a: T, b: T): T {{
            if a < b {{ a }} else {{ b }}
        }}

        fn run(): Number {{
            let m = min_of(Money {{ cents: 700 }}, Money {{ cents: 300 }});
            m.cents
        }}
    "#
    ))
    .expect_output("300");
}

#[test]
fn dictionary_forwards_between_bounded_generics() {
    // A bounded generic calling another bounded generic forwards its own
    // dictionary parameter instead of rebuilding one.
    CliTest::new(format!(
        r#"
        {MONEY}
        fn same<T: Eq>(a: T, b: T): Bool {{
            a.eq(b)
        }}

        fn all_same<T: Eq>(a: T, b: T, c: T): Bool {{
            if same(a, b) {{ same(b, c) }} else {{ false }}
        }}

        fn run(): Bool {{
            all_same(
                Money {{ cents: 5 }},
                Money {{ cents: 5 }},
                Money {{ cents: 5 }},
            )
        }}
    "#
    ))
    .expect_output("true");
}

#[test]
fn multiple_bounds_on_one_param() {
    // `<T: Eq + Ord>` takes two dictionaries; both methods dispatch.
    CliTest::new(format!(
        r#"
        {MONEY}
        fn max_unless_equal<T: Eq + Ord>(a: T, b: T): Bool {{
            if a.eq(b) {{ false }} else {{ a.cmp(b) > 0 }}
        }}

        fn run(): Bool {{
            max_unless_equal(Money {{ cents: 9 }}, Money {{ cents: 3 }})
        }}
    "#
    ))
    .expect_output("true");
}

#[test]
fn bounded_generic_recursion_forwards_dict() {
    // Self-recursion forwards the dictionary through the recursive call
    // (and exercises bounded functions inside a recursive group).
    CliTest::new(format!(
        r#"
        {MONEY}
        fn count_equal<T: Eq>(target: T, a: T, b: T, n: Number): Number {{
            if n <= 0 {{ 0 }} else {{
                let hit = if target.eq(a) {{ 1 }} else {{ 0 }};
                hit + count_equal(target, b, a, n - 1)
            }}
        }}

        fn run(): Number {{
            count_equal(Money {{ cents: 1 }}, Money {{ cents: 1 }}, Money {{ cents: 2 }}, 4)
        }}
    "#
    ))
    .expect_output("2");
}

#[test]
fn unsatisfied_bound_is_an_error() {
    // Calling a bounded generic at a type with no impl reports the bound.
    CliTest::new(
        r#"
        unique(A1B2C3D4-0000-0000-0000-00000000BB02) struct Point { x: Number }

        fn same<T: Eq>(a: T, b: T): Bool {
            a.eq(b)
        }

        fn run(): Bool {
            same(Point { x: 1 }, Point { x: 1 })
        }
    "#,
    )
    .expect_error("is not satisfied");
}

#[test]
fn missing_param_bound_is_an_error() {
    // Using a bound method the parameter doesn't declare points at the
    // missing bound.
    CliTest::new(
        r#"
        fn same<T>(a: T, b: T): Bool {
            a.eq(b)
        }

        fn run(): Bool {
            same(1, 2)
        }
    "#,
    )
    .expect_error("no method `eq` on type parameter `T`");
}

#[test]
fn forwarding_requires_caller_bound() {
    // A generic caller without the bound cannot forward a dictionary it
    // doesn't have.
    CliTest::new(
        r#"
        fn same<T: Eq>(a: T, b: T): Bool {
            a.eq(b)
        }

        fn broken<T>(a: T, b: T): Bool {
            same(a, b)
        }

        fn run(): Bool {
            broken(1, 2)
        }
    "#,
    )
    .expect_error("does not declare the bound `Eq`");
}

#[test]
fn bounded_generic_as_value_is_an_error() {
    // Dictionaries are supplied at call sites; a bounded generic has no
    // first-class value form yet. Binding one generalizes the constrained
    // variable away from its constraint, so the checker reports the
    // unresolvable bound at the reference. (A reference whose type *does*
    // resolve is caught by the compiler's value-position guard instead.)
    CliTest::new(format!(
        r#"
        {MONEY}
        fn same<T: Eq>(a: T, b: T): Bool {{
            a.eq(b)
        }}

        fn run(): Bool {{
            let f = same;
            f(Money {{ cents: 1 }}, Money {{ cents: 2 }})
        }}
    "#
    ))
    .expect_error("constrained by `Eq`");
}

#[test]
fn bounded_inherent_impl_method() {
    // Bounds on an inherent impl block (`impl<T: Eq> List<T>`): the
    // method's dictionary comes from the receiver's element type at the
    // call site.
    CliTest::new(format!(
        r#"
        {MONEY}
        impl<T: Eq> List<T> {{
            fn first_matches(self, needle: T): Bool {{
                match self.get(0) {{
                    Some(item) => item.eq(needle),
                    None => false,
                }}
            }}
        }}

        fn run(): Bool {{
            [Money {{ cents: 4 }}, Money {{ cents: 9 }}]
                .first_matches(Money {{ cents: 4 }})
        }}
    "#
    ))
    .expect_output("true");
}

//! Trait-method-level type-parameter bounds (`fn m<U: Eq>`), and the
//! declaration-site rejection of generic traits and supertraits.

mod common;
use common::*;

// ─────────────────────────────────────────────────────────────────────────────
// Bounds on trait methods' own type parameters
// ─────────────────────────────────────────────────────────────────────────────

/// A trait method with its own trait bound (`fn same<U: Eq>`), implemented for
/// a concrete type and called on a concrete receiver, threads one hidden
/// dictionary per bound as a trailing argument and dispatches through it.
#[test]
fn bounded_trait_method_dispatches_through_a_dictionary() {
    CliTest::new(
        r#"
        use core::traits::Eq;
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000501) trait Tagger {
          fn same<U: Eq>(self, a: U, b: U): Bool;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000502) struct Machine { id: Number }
        impl Tagger for Machine {
          fn same<U: Eq>(self, a: U, b: U): Bool { a.eq(b) }
        }
        fn run(): Bool {
          let m = Machine { id: 1 };
          if m.same(3, 3) { m.same("x", "y") } else { false }
        }
    "#,
    )
    .expect_output("false");
}

/// A bounded generic function that calls a bounded trait method forwards its
/// own dictionary parameter into the method's trailing dictionary slot.
#[test]
fn bounded_fn_forwards_a_dict_into_a_bounded_trait_method() {
    CliTest::new(
        r#"
        use core::traits::Eq;
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000503) trait Tagger {
          fn same<U: Eq>(self, a: U, b: U): Bool;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000504) struct Machine { id: Number }
        impl Tagger for Machine {
          fn same<U: Eq>(self, a: U, b: U): Bool { a.eq(b) }
        }
        fn check<T: Eq>(m: Machine, x: T, y: T): Bool { m.same(x, y) }
        fn run(): Bool { check(Machine { id: 1 }, 5, 5) }
    "#,
    )
    .expect_output("true");
}

/// A conditional impl's own bound and the method's own bound both thread as
/// trailing dictionaries, in the `impl ++ method` order the compiled method
/// allocates them: `impl<T: Eq> ... fn pick<U: Eq>` uses the impl dictionary
/// for `self.a.eq(self.b)` and the method dictionary for `a.eq(b)`.
#[test]
fn conditional_impl_and_method_bounds_thread_combined_dictionaries() {
    CliTest::new(
        r#"
        use core::traits::Eq;
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000505) trait Tagger {
          fn pick<U: Eq>(self, a: U, b: U): Bool;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000506) struct Pair<T> { a: T, b: T }
        impl<T: Eq> Tagger for Pair<T> {
          fn pick<U: Eq>(self, a: U, b: U): Bool { self.a.eq(self.b) && a.eq(b) }
        }
        fn run(): Bool {
          let p = Pair { a: 3, b: 3 };
          p.pick(9, 9)
        }
    "#,
    )
    .expect_output("true");
}

/// A trait method and an impl method may both spell their bound as a trailing
/// `where` clause; it folds into the method's own type-parameter bounds, so the
/// threaded dictionary is identical to the inline `<U: Eq>` form.
#[test]
fn where_clause_on_trait_and_impl_methods() {
    CliTest::new(
        r#"
        use core::traits::Eq;
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000601) trait Tagger {
          fn same<U>(self, a: U, b: U): Bool where U: Eq;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000602) struct Machine { id: Number }
        impl Tagger for Machine {
          fn same<U>(self, a: U, b: U): Bool where U: Eq { a.eq(b) }
        }
        fn run(): Bool {
          let m = Machine { id: 1 };
          m.same(3, 3)
        }
    "#,
    )
    .expect_output("true");
}

/// Calling a bounded trait method with an argument whose type has no matching
/// impl fails the bound solve with a clear diagnostic.
#[test]
fn bounded_trait_method_arg_without_impl_is_rejected() {
    CliTest::new(
        r#"
        use core::traits::Eq;
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000507) trait Tagger {
          fn same<U: Eq>(self, a: U, b: U): Bool;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000508) struct M { id: Number }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000509) struct NoEq { x: Number }
        impl Tagger for M { fn same<U: Eq>(self, a: U, b: U): Bool { a.eq(b) } }
        fn run(): Bool {
          let m = M { id: 1 };
          m.same(NoEq { x: 1 }, NoEq { x: 2 })
        }
    "#,
    )
    .check()
    .expect_error("`NoEq: Eq` is not satisfied");
}

/// An impl method that omits the trait method's declared bound is a conformance
/// error: the bounds are the method's hidden dictionary calling convention.
#[test]
fn impl_method_omitting_a_trait_bound_is_rejected() {
    CliTest::new(
        r#"
        use core::traits::Eq;
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000050A) trait Tagger {
          fn same<U: Eq>(self, a: U, b: U): Bool;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000050B) struct M { id: Number }
        impl Tagger for M {
          fn same<U>(self, a: U, b: U): Bool { true }
        }
    "#,
    )
    .check()
    .expect_error("must declare the same method-level trait bounds");
}

/// An impl method that adds a bound the trait method does not declare is also a
/// conformance error (the reverse mismatch).
#[test]
fn impl_method_adding_a_trait_bound_is_rejected() {
    CliTest::new(
        r#"
        use core::traits::Eq;
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000050C) trait Tagger {
          fn tag<U>(self, u: U): Bool;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000050D) struct M { id: Number }
        impl Tagger for M {
          fn tag<U: Eq>(self, u: U): Bool { true }
        }
    "#,
    )
    .check()
    .expect_error("must declare the same method-level trait bounds");
}

/// Dispatching a bounded trait method through a *rigid type parameter*
/// receiver threads the method's own dictionary through the fixed-arity
/// dictionary slot as an extra trailing runtime argument. Here the method's
/// `U: Eq` dictionary is itself a *forwarded* enclosing dictionary parameter
/// (`DictSource::Param`): `via_param` is generic over `U: Eq`, so `t.same(a,
/// b)` forwards its `U` dictionary into the slot call.
#[test]
fn bounded_trait_method_through_rigid_param_forwards_a_dict() {
    CliTest::new(
        r#"
        use core::traits::Eq;
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000050E) trait Tagger {
          fn same<U: Eq>(self, a: U, b: U): Bool;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000051E) struct Machine { id: Number }
        impl Tagger for Machine {
          fn same<U: Eq>(self, a: U, b: U): Bool { a.eq(b) }
        }
        fn via_param<T: Tagger, U: Eq>(t: T, a: U, b: U): Bool { t.same(a, b) }
        fn run(): Bool { via_param(Machine { id: 1 }, 5, 5) }
    "#,
    )
    .expect_output("true");
}

/// A conditional impl of a trait with a bounded method can serve as a
/// dictionary source: its `DictSource::Generic` slot closure accepts the
/// method's own dictionary as an extra trailing argument, forwarded after the
/// impl's captured inner dictionaries. Here `impl<T: Eq> Tagger for Pair<T>`'s
/// `same<U: Eq>` uses the impl dictionary for `self.a.eq(self.b)` (over `T =
/// Number`) and the method dictionary for `a.eq(b)` (over `U = String`) — two
/// distinct dictionaries proving the `impl ++ method` order is preserved.
#[test]
fn conditional_impl_of_bounded_method_trait_serves_as_a_dict() {
    CliTest::new(
        r#"
        use core::traits::Eq;
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF0000050F) trait Tagger {
          fn same<U: Eq>(self, a: U, b: U): Bool;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000510) struct Pair<T> { a: T, b: T }
        impl<T: Eq> Tagger for Pair<T> {
          fn same<U: Eq>(self, a: U, b: U): Bool { self.a.eq(self.b) && a.eq(b) }
        }
        fn needs<X: Tagger>(x: X): Bool { x.same("hi", "hi") }
        fn run(): Bool { needs(Pair { a: 2, b: 2 }) }
    "#,
    )
    .expect_output("true");
}

// ─────────────────────────────────────────────────────────────────────────────
// Generic-trait declaration limits and supertraits: rejected at declaration
// ─────────────────────────────────────────────────────────────────────────────

/// A trait-level type parameter is a pure argument slot: a bound on it
/// (`trait Container<T: Eq>`) is rejected at the declaration site — bounds
/// belong on the impls and functions that use the trait.
#[test]
fn bounded_trait_type_param_is_rejected_at_declaration() {
    CliTest::new(
        r#"
        use core::traits::Eq;
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000511) trait Container<T: Eq> {
          fn size(self): Number;
        }
    "#,
    )
    .check()
    .expect_error("declares a bound on its type parameter");
}

/// A trait with a supertrait clause (`trait Sub with Base`) is not yet
/// supported, rejected with a clear declaration-site diagnostic.
#[test]
fn supertrait_is_rejected_at_declaration() {
    CliTest::new(
        r#"
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000512) trait Base {
          fn b(self): Number;
        }
        unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000513) trait Sub with Base {
          fn s(self): Number;
        }
    "#,
    )
    .check()
    .expect_error("supertraits are not supported yet");
}

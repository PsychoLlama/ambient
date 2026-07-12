//! User-declared generic nominal structs: construction, projection,
//! generic-arg inference, inherent impls, and effect-polymorphic args.

mod common;
use common::*;

#[test]
fn test_generic_struct_construct_project_and_run() {
    // The canonical repro: a two-parameter generic struct, constructed with
    // inferred type arguments, passed to a function that annotates the applied
    // type and projects both fields.
    CliTest::new(
        r#"
        unique(AAAAAAAA-0000-0000-0000-000000000005) struct Box3<A, B> { a: A, b: B }

        fn use_box(p: Box3<Number, Number>): Number {
          p.a + p.b
        }

        pub fn run(): Number {
          use_box(Box3 { a: 2, b: 3 })
        }
    "#,
    )
    .expect_output("5");
}

#[test]
fn test_generic_struct_projection_through_annotated_param() {
    // Projection through an annotated generic-struct parameter yields the
    // substituted field type (here `String`, concatenated).
    CliTest::new(
        r#"
        unique(AAAAAAAA-0000-0000-0000-000000000006) struct Pair<A, B> { first: A, second: B }

        fn first_of(p: Pair<String, Number>): String {
          p.first
        }

        pub fn run(): String {
          first_of(Pair { first: "hi", second: 7 })
        }
    "#,
    )
    .expect_output("hi");
}

#[test]
fn test_generic_struct_construction_arg_mismatch_is_error() {
    // Constructing where `Box3<Number, Number>` is expected but a field is a
    // String is a type error (the inferred args must unify with the annotation).
    CliTest::new(
        r#"
        unique(AAAAAAAA-0000-0000-0000-000000000007) struct Box3<A, B> { a: A, b: B }

        fn use_box(p: Box3<Number, Number>): Number {
          p.a + p.b
        }

        pub fn run(): Number {
          use_box(Box3 { a: 2, b: "x" })
        }
    "#,
    )
    .check()
    .expect_failure();
}

#[test]
fn test_generic_struct_arg_mismatch_two_applications() {
    // Two applied forms of the same struct with different args do not unify.
    CliTest::new(
        r#"
        unique(AAAAAAAA-0000-0000-0000-000000000008) struct Box3<A, B> { a: A, b: B }

        fn takes_ns(p: Box3<Number, String>): Number { p.a }

        pub fn run(): Number {
          takes_ns(Box3 { a: 1, b: 2 })
        }
    "#,
    )
    .check()
    .expect_failure();
}

#[test]
fn test_generic_struct_inherent_impl() {
    // `impl<T> Wrap<T>` over a generic user struct: the applied head must reach
    // the inherent-impl target filter with its uuid so method dispatch works.
    CliTest::new(
        r#"
        unique(AAAAAAAA-0000-0000-0000-000000000009) struct Wrap<T> { value: T }

        impl<T> Wrap<T> {
          fn get(self): T { self.value }
        }

        pub fn run(): Number {
          let w = Wrap { value: 99 };
          w.get()
        }
    "#,
    )
    .expect_output("99");
}

#[test]
fn test_generic_struct_effect_polymorphic_field() {
    // A generic struct whose argument is a function type carrying an ability
    // row, used inside an effect-polymorphic function.
    CliTest::new(
        r#"
        unique(AAAAAAAA-0000-0000-0000-00000000000A) struct Pair<A, B> { first: A, second: B }

        fn run_first<E!>(p: Pair<() -> () with E, Number>): Number with E {
          let f = p.first;
          f();
          p.second
        }

        pub fn run(): Number {
          run_first(Pair { first: () => (), second: 4 })
        }
    "#,
    )
    .expect_output("4");
}

#[test]
fn test_generic_enum_still_works() {
    // Spot-check: a generic enum with a payload constructed and matched still
    // projects/dispatches correctly (guards against a shared regression).
    CliTest::new(
        r#"
        unique(AAAAAAAA-0000-0000-0000-00000000000B) enum Wrapper<T> { Wrapped(T) }

        fn unwrap(w: Wrapper<Number>): Number {
          match w {
            Wrapped(n) => n,
          }
        }

        pub fn run(): Number {
          unwrap(Wrapped(77))
        }
    "#,
    )
    .expect_output("77");
}

//! Ability inference for private functions, duplicate/ambiguous impl errors, and Option-returning partial intrinsics.

mod common;
use common::*;

// ─────────────────────────────────────────────────────────────────────────────
// Ability inference for private functions
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_private_function_ability_inference() {
    // Private helpers need no `with` annotations: their abilities are
    // inferred from their bodies (including through mutual recursion and
    // calls to functions defined later) and propagate to callers.
    CliTest::new(
        r#"
        pub fn run(): () with core::system::Stdio {
            ping(2);
            helper_outer();
        }

        fn ping(n: Number) {
            if n > 0 { core::system::Stdio::out!("ping"); pong(n - 1); } else { () }
        }

        fn pong(n: Number) {
            if n > 0 { core::system::Stdio::out!("pong"); ping(n - 1); } else { () }
        }

        fn helper_inner() { core::system::Stdio::out!("inner"); }
        fn helper_outer() { helper_inner(); }
    "#,
    )
    .expect_output("ping");
}

#[test]
fn test_public_function_must_declare_inferred_abilities() {
    // Inferred abilities from private helpers still count against a public
    // function's declarations — declaring pure while transitively performing
    // Stdio is an error, even when the helper is defined after the caller.
    CliTest::new(
        r#"
        pub fn run(): () {
            leaky();
        }

        fn leaky() {
            core::system::Stdio::out!("leak");
        }
    "#,
    )
    .expect_error("uses ability `Stdio` but doesn't declare it");
}

#[test]
fn test_duplicate_impl_is_error() {
    // A trait may be implemented at most once per type: the impl-method
    // dispatch symbol is derived from (type uuid, trait, method), so a
    // second impl would collide in the content-addressed store.
    CliTest::new(
        r#"
        unique(AAAAAAAA-BBBB-4CCC-8DDD-000000000012) trait Show {
            fn show(self): Number;
        }

        unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00003333) struct Id { value: Number }

        impl Show for Id {
            fn show(self): Number { self.value }
        }

        impl Show for Id {
            fn show(self): Number { self.value * 2 }
        }

        fn run(): Number {
            let i = Id { value: 1 };
            i.show()
        }
    "#,
    )
    .expect_error("duplicate implementation of trait `Show`");
}

#[test]
fn test_ambiguous_method_is_error() {
    // Two different traits implemented for the same type both provide a
    // method named `render`; a bare method call cannot choose between them.
    CliTest::new(
        r#"
        unique(AAAAAAAA-BBBB-4CCC-8DDD-000000000013) trait Html {
            fn render(self): Number;
        }

        unique(AAAAAAAA-BBBB-4CCC-8DDD-000000000014) trait Text {
            fn render(self): Number;
        }

        unique(AAAABBBB-CCCC-DDDD-EEEE-FFFF00004444) struct Page { id: Number }

        impl Html for Page {
            fn render(self): Number { self.id }
        }

        impl Text for Page {
            fn render(self): Number { self.id * 2 }
        }

        fn run(): Number {
            let p = Page { id: 1 };
            p.render()
        }
    "#,
    )
    .expect_error("render");
}

// ─────────────────────────────────────────────────────────────────────────────
// Honest Partial Intrinsics (Option-returning accessors)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_list_accessors_return_option() {
    // `List::get`/`head`/`first`/`last` return Option: `Some` on a hit,
    // `None` when the element is missing — never a substituted `()`.
    let (dir, pkg) = temp_package(
        r#"
        pub fn run(): String {
            let hit = match [1, 2, 3].get(1) {
                Some(v) => v,
                None => -1,
            };
            let miss = match [].head() {
                Some(v) => v,
                None => -1,
            };
            let first = [7, 8].first().unwrap_or(0);
            let last = [7, 8].last().unwrap_or(0);
            core::convert::to_string(hit) + " " + core::convert::to_string(miss)
                + " " + core::convert::to_string(first) + " " + core::convert::to_string(last)
        }
        "#,
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("2 -1 7 8"));
    drop(dir);
}

#[test]
fn test_list_option_chains_through_method_combinators() {
    // The Option flows through the inherent method forms:
    // `xs.head().unwrap_or(...)`, `xs.get(i).map(...)`.
    let (dir, pkg) = temp_package(
        r"
        pub fn run(): Number {
            let xs = [10, 20, 30];
            let empty: List<Number> = [];
            xs.head().unwrap_or(0)
                + xs.get(2).map((v: Number) => v * 2).unwrap_or(0)
                + xs.last().unwrap_or(0)
                + empty.head().unwrap_or(1000)
        }
        ",
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    // 10 + 60 + 30 + 1000
    assert!(String::from_utf8_lossy(&output.stdout).contains("1100"));
    drop(dir);
}

#[test]
fn number_and_bool_parse_through_try_from() {
    // The low-level parser externs are module-private; the public surface
    // is the prelude `TryFrom<String>` impls (and the `try_into` bridge).
    let (dir, pkg) = temp_package(
        r#"
        pub fn run(): String {
            let good = Number::try_from("42.5").unwrap_or(0);
            let bad = match Number::try_from("abc") {
                Ok(_) => "ok",
                Err(_) => "err",
            };
            let yes = Bool::try_from("true").unwrap_or(false);
            let junk: Result<Bool, String> = "maybe".try_into();
            core::convert::to_string(good) + " " + bad + " "
                + core::convert::to_string(yes) + " "
                + core::convert::to_string(junk.is_err())
        }
        "#,
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("42.5 err true true"));
    drop(dir);
}

#[test]
fn test_map_get_returns_option() {
    let (dir, pkg) = temp_package(
        r#"
        pub fn run(): String {
            let m = Map::empty().insert("a", 1);
            let hit = m.get("a").unwrap_or(0);
            let miss = match m.get("b") {
                Some(_) => "some",
                None => "none",
            };
            core::convert::to_string(hit) + " " + miss
        }
        "#,
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 none"));
    drop(dir);
}

#[test]
fn test_map_and_set_inherent_methods() {
    // `Map`/`Set` are nominal container types with inherent-method companions:
    // `map.insert(k, v).get(k)` and `set.insert(v).contains(v)` dispatch
    // through the same uuid-keyed machinery as `list.map(f)`.
    let (dir, pkg) = temp_package(
        r#"
        pub fn run(): String {
            let m = Map::empty().insert("a", 1).insert("b", 2);
            let hit = m.get("a").unwrap_or(0);
            let n = m.length();

            let s = Set::empty().insert(7).insert(7).insert(9);
            let has = s.contains(9);
            let size = s.length();

            core::convert::to_string(hit) + " "
                + core::convert::to_string(n) + " "
                + core::convert::to_string(has) + " "
                + core::convert::to_string(size)
        }
        "#,
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 2 true 2"));
    drop(dir);
}

#[test]
fn test_string_index_of_returns_option() {
    let (dir, pkg) = temp_package(
        r#"
        pub fn run(): String {
            let found = "hello world".index_of("wor").unwrap_or(-1);
            let missing = "hello".index_of("xyz").is_none();
            core::convert::to_string(found) + " " + core::convert::to_string(missing)
        }
        "#,
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("6 true"));
    drop(dir);
}

#[test]
fn test_string_interpolation_with_braced_block_and_nested_string() {
    // Regression: a brace-delimited construct (here a lambda whose body is a
    // block) inside a `${...}` interpolation used to truncate the
    // interpolation at the block's closing `}`, swallowing the rest of the
    // expression as literal text (`expected RParen, found StringEnd`). A
    // nested string literal (`"hi"`) inside the interpolation must also lex.
    CliTest::new(
        r#"
        fn pick(f: () -> Number, n: Number): Number {
            f() + n
        }

        fn tag(s: String, n: Number): String {
            s
        }

        pub fn run(): () with core::system::Stdio {
            core::system::Stdio::out!("out: ${ tag("hi", pick(() => { let x = 2; x }, 40)) }");
        }
    "#,
    )
    .expect_output("out: hi");
}

// ─────────────────────────────────────────────────────────────────────────────
// Type inference for unannotated private functions (body ↔ scheme sharing)
// ─────────────────────────────────────────────────────────────────────────────
//
// An unannotated parameter or return position is monomorphic: one type
// variable shared by the function's body and every call site. The body's
// constraints therefore reach callers (and vice versa) — before this,
// bodies and call sites ran on disjoint fresh variables, and
// `fn g(x) { x + 1 }; g(true)` checked clean and executed `true + 1`.

#[test]
fn test_body_pins_unannotated_param_rejecting_mismatched_caller() {
    // Callee checked first: the body pins `x: Number`, so the call site
    // reports the mismatch.
    CliTest::new(
        r#"
        fn g(x) { x + 1 }

        pub fn run(): Number {
            g(true)
        }
        "#,
    )
    .check()
    .expect_error("type mismatch: expected `Number`, found `Bool`");
}

#[test]
fn test_caller_pins_unannotated_param_rejecting_mismatched_body() {
    // Caller checked first: the call site pins `x: Bool`, so the body's
    // `x + 1` reports the mismatch (in function `g`).
    CliTest::new(
        r#"
        pub fn run(): Number {
            g(true)
        }

        fn g(x) { x + 1 }
        "#,
    )
    .check()
    .expect_error("type mismatch: expected `Bool`, found `Number`");
}

#[test]
fn test_unannotated_return_type_flows_to_callers() {
    CliTest::new(
        r#"
        fn make() { "hello" }

        pub fn run(): Number {
            make()
        }
        "#,
    )
    .check()
    .expect_error("type mismatch");
}

#[test]
fn test_unannotated_recursive_function_infers_cleanly() {
    // Self-recursion constrains the shared variables consistently: the
    // recursive call's type IS the scheme's return variable.
    CliTest::new(
        r#"
        fn count(n) {
            if n == 0 { 0 } else { count(n - 1) }
        }

        pub fn run(): Number {
            count(3)
        }
        "#,
    )
    .check()
    .expect_success();
}

#[test]
fn test_generic_function_unannotated_position_must_be_annotated() {
    // The body forces `b: T` and the return to `T`. A monomorphic shared
    // variable cannot carry a quantified parameter, so both positions get a
    // dedicated "annotate it" error instead of leaking a rigid `T` into
    // call sites.
    CliTest::new(
        r#"
        fn pick<T>(a: T, b) {
            if true { a } else { b }
        }

        pub fn run(): Number {
            pick(1, 2)
        }
        "#,
    )
    .check()
    .expect_error("must be annotated in the signature");
}

#[test]
fn test_public_fn_requires_full_signature_annotations() {
    // A `pub fn` signature is the cross-module contract: importing modules
    // rebuild the scheme from the written annotations alone, so inference
    // can never fill an omitted type for foreign callers.
    CliTest::new(
        r#"
        pub fn f(x) {
            x + 1
        }
        "#,
    )
    .check()
    .expect_error("public function `f` must declare");
}

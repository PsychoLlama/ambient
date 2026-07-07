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
        trait Show {
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
        trait Html {
            fn render(self): Number;
        }

        trait Text {
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
            let hit = match core::collections::List::get([1, 2, 3], 1) {
                Some(v) => v,
                None => 0 - 1,
            };
            let miss = match core::collections::List::head([]) {
                Some(v) => v,
                None => 0 - 1,
            };
            let first = core::collections::List::first([7, 8]).unwrap_or(0);
            let last = core::collections::List::last([7, 8]).unwrap_or(0);
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
fn test_parse_number_and_parse_bool_return_option() {
    let (dir, pkg) = temp_package(
        r#"
        pub fn run(): String {
            let good = core::convert::parse_number("42.5").unwrap_or(0);
            let bad = match core::convert::parse_number("abc") {
                Some(_) => "some",
                None => "none",
            };
            let yes = core::convert::parse_bool("true").unwrap_or(false);
            let junk = core::convert::parse_bool("maybe").is_none();
            core::convert::to_string(good) + " " + bad + " "
                + core::convert::to_string(yes) + " " + core::convert::to_string(junk)
        }
        "#,
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("42.5 none true true"));
    drop(dir);
}

#[test]
fn test_map_get_returns_option() {
    let (dir, pkg) = temp_package(
        r#"
        pub fn run(): String {
            let m = core::collections::map::insert(core::collections::map::empty(), "a", 1);
            let hit = core::collections::map::get(m, "a").unwrap_or(0);
            let miss = match core::collections::map::get(m, "b") {
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
fn test_string_index_of_returns_option() {
    let (dir, pkg) = temp_package(
        r#"
        pub fn run(): String {
            let found = core::primitives::String::index_of("hello world", "wor").unwrap_or(0 - 1);
            let missing = core::primitives::String::index_of("hello", "xyz").is_none();
            core::convert::to_string(found) + " " + core::convert::to_string(missing)
        }
        "#,
    );
    let output = ambient_cmd().arg("run").arg(&pkg).output().expect("run");
    assert!(output.status.success(), "run failed: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("6 true"));
    drop(dir);
}

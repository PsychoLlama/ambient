//! Trait-reference resolution obeys the `Fqn` invariant: impl headers and
//! bounds canonicalize through the resolve pass, so a foreign scheme's bound
//! resolves in its defining module (no consumer-scope shadow), same-named
//! traits from different modules stay distinct, and qualified bounds work.

mod common;
use common::*;

#[test]
fn test_foreign_bound_resolves_against_defining_module() {
    // A bounded generic's bound must resolve to the trait its *defining*
    // module named — never re-resolved in the caller's scope. The caller
    // declares its own trait of the same name (a decoy); the imported
    // `describe`'s `T: Pretty` bound must still mean the *search* module's
    // `Pretty`, which `Money` implements. Under the old name-scoped lenient
    // resolution the caller's local `Pretty` would shadow it and the
    // dictionary solve would fail.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "search.ab",
            r#"
            pub unique(BBBB0000-0000-4000-8000-000000000101) trait Pretty {
                fn pretty(self): Number;
            }

            pub fn describe<T: Pretty>(x: T): Number { x.pretty() }
            "#,
        ),
        (
            "money.ab",
            r#"
            use pkg::search::{Pretty};

            pub unique(BBBB0000-0000-4000-8000-000000000102) struct Money { cents: Number }

            impl Pretty for Money {
                fn pretty(self): Number { self.cents }
            }

            pub fn make(cents: Number): Money { Money { cents: cents } }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::search::describe;
            use pkg::money::make;

            // A decoy trait of the same bare name, distinct identity. It must
            // not capture `describe`'s foreign `T: Pretty` bound.
            pub unique(BBBB0000-0000-4000-8000-000000000103) trait Pretty {
                fn pretty(self): Number;
            }

            pub fn run(): Number { describe(make(42)) }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("42"),
        "expected 42 in output, got: {stdout}"
    );
}

#[test]
fn test_two_same_named_traits_stay_distinct() {
    // Two modules each declare a trait named `Tag` with a distinct identity,
    // each with its own bounded generic and an implementing type. A caller
    // importing both bounded functions dispatches each through the right
    // trait — no collision. The old build-global unique-name fallback saw
    // two `Tag`s and resolved neither (ambiguous), so this used to fail.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "a.ab",
            r#"
            pub unique(CCCC0000-0000-4000-8000-000000000201) trait Tag {
                fn tag(self): Number;
            }

            pub unique(CCCC0000-0000-4000-8000-000000000202) struct Ay { v: Number }

            impl Tag for Ay { fn tag(self): Number { self.v } }

            pub fn a_tag<T: Tag>(x: T): Number { x.tag() }
            pub fn make_a(v: Number): Ay { Ay { v: v } }
            "#,
        ),
        (
            "b.ab",
            r#"
            pub unique(CCCC0000-0000-4000-8000-000000000203) trait Tag {
                fn tag(self): Number;
            }

            pub unique(CCCC0000-0000-4000-8000-000000000204) struct Bee { v: Number }

            impl Tag for Bee { fn tag(self): Number { self.v * 10 } }

            pub fn b_tag<T: Tag>(x: T): Number { x.tag() }
            pub fn make_b(v: Number): Bee { Bee { v: v } }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::a::{a_tag, make_a};
            use pkg::b::{b_tag, make_b};

            pub fn run(): Number { a_tag(make_a(5)) + b_tag(make_b(5)) }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // 5 + 50
    assert!(
        stdout.contains("55"),
        "expected 55 in output, got: {stdout}"
    );
}

#[test]
fn test_qualified_trait_bound() {
    // A bound may name its trait by a fully-qualified path
    // (`T: pkg::lib::Q`) without importing it — the resolve pass
    // canonicalizes the path to the trait's `Fqn`, so the dictionary solves
    // against the foreign impl. `main` never `use`s `Q`.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "lib.ab",
            r#"
            pub unique(DDDD0000-0000-4000-8000-000000000301) trait Q {
                fn q(self): Number;
            }

            pub unique(DDDD0000-0000-4000-8000-000000000302) struct W { v: Number }

            impl Q for W { fn q(self): Number { self.v } }

            pub fn make(v: Number): W { W { v: v } }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::lib::make;

            fn describe<T: pkg::lib::Q>(x: T): Number { x.q() }

            pub fn run(): Number { describe(make(7)) }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("7"), "expected 7 in output, got: {stdout}");
}

#[test]
fn test_public_generic_fn_bound_on_trait_in_another_module() {
    // A *public* generic function whose bound names a trait declared in a
    // *separate* module. This is the case a co-located generic fn (the
    // workaround in the other tests here) can't exercise: foreign *public*
    // functions build their schemes eagerly while checking the trait's own
    // module — before that module's local trait registration ran — so the
    // bound resolved against an empty `fqn_to_uuid` and reported a spurious
    // `unknown trait`, attributed (with a garbled span) to the trait's
    // definition site. The four spellings below are the full blast radius:
    //
    //   * `traits.ab`  — trait only, so its module has no other reason to be
    //     registered before the generic module is checked;
    //   * `use_gen.ab` — `use`-imported bare bound;
    //   * `path_gen.ab`— fully-qualified bound, never `use`d;
    //   * `where_gen.ab`— `where`-clause bound;
    //   * `main.ab`    — a *third* module supplying the concrete impl and
    //     instantiating each generic, so dictionaries pass across modules.
    let (_dir, pkg) = temp_multi_package(&[
        (
            "traits.ab",
            r#"
            pub unique(EEEE0000-0000-4000-8000-000000000401) trait Describe {
                fn describe(self): Number;
            }
            "#,
        ),
        (
            "use_gen.ab",
            r#"
            use pkg::traits::Describe;
            pub fn via_use<T: Describe>(x: T): Number { x.describe() }
            "#,
        ),
        (
            "path_gen.ab",
            r#"
            pub fn via_path<T: pkg::traits::Describe>(x: T): Number { x.describe() }
            "#,
        ),
        (
            "where_gen.ab",
            r#"
            use pkg::traits::Describe;
            pub fn via_where<T>(x: T): Number where T: Describe { x.describe() }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::traits::Describe;
            use pkg::use_gen::via_use;
            use pkg::path_gen::via_path;
            use pkg::where_gen::via_where;

            pub unique(EEEE0000-0000-4000-8000-000000000402) struct Widget { x: Number }
            impl Describe for Widget { fn describe(self): Number { self.x } }

            pub fn run(): Number {
                via_use(Widget { x: 100 })
                    + via_path(Widget { x: 20 })
                    + via_where(Widget { x: 3 })
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // 100 + 20 + 3
    assert!(
        stdout.contains("123"),
        "expected 123 in output, got: {stdout}"
    );
}

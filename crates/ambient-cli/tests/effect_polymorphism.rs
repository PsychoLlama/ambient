//! Effect polymorphism (Phase 1): ordinary functions generic over an
//! ability (row) variable declared `E!`. `with E` refers to that variable in
//! the function's own `with` clause and in function-type annotations; the
//! variable propagates a callee's effects to the caller and instantiates
//! independently per call site. Effects are erased before compilation, so
//! these are all check-time (and one run-time) properties.

mod common;

use common::{CliTest, ambient_cmd, temp_multi_package};

/// The canonical example: an `E!`-polymorphic function forwards an effectful
/// thunk. A caller passing an effectful lambda must itself declare (or
/// handle) the ability; a caller passing a pure lambda needs nothing.
#[test]
fn effect_variable_propagates_and_instantiates_pure() {
    CliTest::new(
        r#"
        pub fn example<E!>(t: () -> String with E): String with E {
          t()
        }

        pub fn effectful_caller(): String with core::system::Stdio {
          example(() => { core::system::Stdio::out!("hi"); "done" })
        }

        pub fn pure_caller(): String {
          example(() => "pure")
        }
        "#,
    )
    .check()
    .expect_success();
}

/// A pure caller passing a pure lambda: `E` instantiates to the empty row, so
/// the whole call is pure and requires no abilities.
#[test]
fn pure_lambda_leaves_the_caller_pure() {
    CliTest::new(
        r#"
        pub fn run_thunk<E!>(t: () -> Number with E): Number with E {
          t()
        }

        pub fn total(): Number {
          run_thunk(() => 41)
        }
        "#,
    )
    .check()
    .expect_success();
}

/// Handling at the call boundary discharges the ability, so the enclosing
/// function is pure — and the program actually runs.
#[test]
fn handling_at_the_call_boundary_discharges_the_ability() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000E1) ability Beep {
          fn beep(): Number { 0 }
        }

        pub fn example<E!>(t: () -> Number with E): Number with E {
          t()
        }

        pub fn run(): Number {
          with { Beep::beep() => resume(5) } handle example(() => Beep::beep!())
        }
        "#,
    )
    .expect_output("5");
}

/// A mixed row `with Stdio, E`: the body may perform the concrete `Stdio`
/// ability *and* call the `E`-carrying parameter.
#[test]
fn mixed_concrete_and_variable_row() {
    CliTest::new(
        r#"
        pub fn logged<E!>(t: () -> String with E): String with core::system::Stdio, E {
          core::system::Stdio::out!("before");
          t()
        }
        "#,
    )
    .check()
    .expect_success();
}

/// An effectful function type sitting inside a generic argument list, ahead
/// of a plain type argument (`Map<() -> Number with E, Number>`): the parser
/// must read two generic arguments — the effectful function type and
/// `Number` — and the `with E` row inside the argument must resolve to the
/// enclosing function's ability variable like any other function-type row.
#[test]
fn effectful_function_type_inside_generic_argument_resolves() {
    CliTest::new(
        r#"
        pub fn indexed<E!>(m: Map<() -> Number with E, Number>): Number with E {
          0
        }
        "#,
    )
    .check()
    .expect_success();
}

/// A body performing an ability outside its declared row (`with E` only, body
/// performs `Stdio` directly) is rejected — the concrete effect escapes.
#[test]
fn ability_outside_the_declared_row_is_rejected() {
    CliTest::new(
        r#"
        pub fn leaky<E!>(t: () -> String with E): String with E {
          core::system::Stdio::out!("leak");
          t()
        }
        "#,
    )
    .check()
    .expect_error("uses ability `Stdio` but doesn't declare it");
}

/// Two ability variables may be declared and each is independently usable in
/// its own function-type position (the single-tail representation means they
/// never share one row).
#[test]
fn two_ability_variables_coexist() {
    CliTest::new(
        r#"
        pub fn use_e<E!, F!>(a: () -> Number with E): Number with E { a() }
        pub fn use_f<E!, F!>(a: () -> Number with F): Number with F { a() }
        "#,
    )
    .check()
    .expect_success();
}

/// Two ability variables in one `with` row is an error — a row has a single
/// polymorphic tail.
#[test]
fn two_variables_in_one_row_is_rejected() {
    CliTest::new(
        r#"
        pub fn bad<E!, F!>(a: () -> Number with E): Number with E, F {
          a()
        }
        "#,
    )
    .check()
    .expect_error("at most one ability variable");
}

/// Trait bounds on an ability variable are rejected — it names an effect row,
/// not a type.
#[test]
fn bounds_on_an_ability_variable_are_rejected() {
    CliTest::new(
        r#"
        pub fn bad<E!: core::cmp::Eq>(a: () -> Number with E): Number with E { a() }
        "#,
    )
    .check()
    .expect_error("cannot have trait bounds");
}

/// Using an ability variable where a type is expected is an error.
#[test]
fn ability_variable_used_as_a_type_is_rejected() {
    CliTest::new(
        r#"
        pub fn bad<E!>(x: E): Number with E { 0 }
        "#,
    )
    .check()
    .expect_error("is an ability variable, not a type");
}

/// An effect-polymorphic function calling another effect-polymorphic
/// function: the row variable flows through the call unchanged.
#[test]
fn row_variable_flows_through_a_polymorphic_call() {
    CliTest::new(
        r#"
        pub fn inner<E!>(t: () -> Number with E): Number with E { t() }
        pub fn outer<E!>(t: () -> Number with E): Number with E { inner(t) }
        "#,
    )
    .check()
    .expect_success();
}

/// A self-recursive effect-polymorphic function: the recursive call carries
/// the same row variable.
#[test]
fn self_recursive_effect_polymorphic_function() {
    CliTest::new(
        r#"
        pub fn repeat<E!>(n: Number, t: () -> Number with E): Number with E {
          if n == 0 { t() } else { repeat(n - 1, t) }
        }
        "#,
    )
    .check()
    .expect_success();
}

/// Two call sites of the same polymorphic function instantiate independently
/// — one pure, one effectful — within a single body.
#[test]
fn independent_instantiation_at_two_call_sites() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000E2) ability Beep {
          fn beep(): Number { 0 }
        }

        pub fn id<E!>(t: () -> Number with E): Number with E { t() }

        pub fn mix(): Number with Beep {
          let pure = id(() => 1);
          let effectful = id(() => Beep::beep!());
          pure + effectful
        }
        "#,
    )
    .check()
    .expect_success();
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 2: effect-polymorphic ability method declarations
// ─────────────────────────────────────────────────────────────────────────────

/// A canonical effect-polymorphic ability method: `run_it<T, E!>` forwards an
/// effectful thunk. Performing it with a *pure* lambda needs only the ability
/// itself — `E` instantiates to the empty row.
#[test]
fn ability_method_perform_with_pure_lambda() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000F1) ability Runner {
          fn run_it<T, E!>(body: () -> T with E): T { body() }
        }

        pub fn use_pure(): Number with Runner {
          Runner::run_it!(() => 41)
        }
        "#,
    )
    .check()
    .expect_success();
}

/// Performing the method with an *effectful* lambda still requires only the
/// ability's own row — the lambda's effects bind the fresh row variable but do
/// not join the caller's required abilities just from being passed (they run
/// wherever the handler/default invokes the thunk). The caller declares
/// `Runner` only, not `Stdio`.
#[test]
fn ability_method_effectful_lambda_does_not_leak_into_caller() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000F2) ability Runner {
          fn run_it<T, E!>(body: () -> T with E): T { body() }
        }

        pub fn use_effectful(): Number with Runner {
          Runner::run_it!(() => { core::system::Stdio::out!("hi"); 1 })
        }
        "#,
    )
    .check()
    .expect_success();
}

/// Unhandled, the perform runs the method's default implementation, which
/// invokes the (pure) lambda — the program actually runs and yields its value.
#[test]
fn ability_method_unhandled_runs_default_over_the_lambda() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000F3) ability Runner {
          fn run_it<T, E!>(body: () -> T with E): T { body() }
        }

        pub fn run(): Number with Runner {
          Runner::run_it!(() => 7)
        }
        "#,
    )
    .expect_output("7");
}

/// A handler arm binds the function-typed parameter at the method's declared
/// (instantiated) type and may call it; `resume` feeds the method's return
/// type. The whole program runs, the arm calling `body()` and resuming with
/// its result.
#[test]
fn handler_arm_calls_the_bound_thunk_and_resumes() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000F4) ability Runner {
          fn run_it<T, E!>(body: () -> T with E): T { body() }
        }

        pub fn run(): Number {
          with { Runner::run_it(body) => resume(body()) } handle Runner::run_it!(() => 7)
        }
        "#,
    )
    .expect_output("7");
}

/// The handler arm's *own* effects flow to the enclosing context: an arm that
/// performs a concrete `Stdio` needs the enclosing function to provide it.
/// Positive: the enclosing function declares `Stdio`.
#[test]
fn handler_arm_effects_flow_to_enclosing_context() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000F5) ability Runner {
          fn run_it<T, E!>(body: () -> T with E): T { body() }
        }

        pub fn good(): Number with core::system::Stdio {
          with { Runner::run_it(body) => { core::system::Stdio::out!("side"); resume(body()) } }
          handle Runner::run_it!(() => 7)
        }
        "#,
    )
    .check()
    .expect_success();
}

/// Negative counterpart: the same arm in a pure (public) enclosing function is
/// rejected — its concrete `Stdio` effect escapes undeclared.
#[test]
fn handler_arm_concrete_effect_must_be_declared() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000F6) ability Runner {
          fn run_it<T, E!>(body: () -> T with E): T { body() }
        }

        pub fn bad(): Number {
          with { Runner::run_it(body) => { core::system::Stdio::out!("side"); resume(body()) } }
          handle Runner::run_it!(() => 7)
        }
        "#,
    )
    .check()
    .expect_error("uses ability `Stdio` but doesn't declare it");
}

/// A non-function argument where the method expects `() -> T with E` is a
/// compile error at the perform site (the declared parameter shape is
/// enforced).
#[test]
fn ability_method_wrong_shaped_argument_is_rejected() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000F7) ability Runner {
          fn run_it<T, E!>(body: () -> T with E): T { body() }
        }

        pub fn bad(): Number with Runner {
          Runner::run_it!(42)
        }
        "#,
    )
    .check()
    .expect_failure();
}

/// A row variable in the *ability-level* dependency row is out of scope: the
/// dependency list names abilities, and a method's `E!` is not one — so `with
/// E` on the declaration is an unknown ability, reported clearly.
#[test]
fn row_variable_in_ability_dependency_row_is_rejected() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-0000000000F8) ability Runner with E {
          fn run_it<T, E!>(body: () -> T with E): T { body() }
        }
        "#,
    )
    .check()
    .expect_error("unknown ability: `E`");
}

/// Cross-module: an effect-polymorphic ability declared in one module is
/// performed in another, both through `use` (bare) and fully qualified. The
/// module-system access rule holds for effect-polymorphic abilities too.
#[test]
fn cross_module_effect_polymorphic_ability() {
    let (_dir, pkg) = temp_multi_package(&[
        (
            "effects.ab",
            r#"
            pub unique(AB000000-0000-0000-0000-0000000000F9) ability Runner {
              fn run_it<T, E!>(body: () -> T with E): T { body() }
            }
            "#,
        ),
        (
            "main.ab",
            r#"
            use pkg::effects::Runner;

            fn via_use(): Number with Runner {
              Runner::run_it!(() => 3)
            }

            fn via_qualified(): Number with pkg::effects::Runner {
              pkg::effects::Runner::run_it!(() => 4)
            }

            pub fn run(): Number with Runner {
              via_use() + via_qualified()
            }
            "#,
        ),
    ]);

    let output = ambient_cmd()
        .arg("run")
        .arg(&pkg)
        .output()
        .expect("failed to run ambient");
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains('7'), "expected 7 in output, got: {stdout}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 3: effect-polymorphic inherent impl methods
// ─────────────────────────────────────────────────────────────────────────────

/// The canonical inherent-method case: a method on a user type forwards an
/// effectful lambda, propagating its row to the caller. An effectful lambda
/// makes the caller declare (or handle) the ability; a pure lambda leaves a
/// pure caller pure — `E` instantiates to the empty row per call site.
#[test]
fn inherent_method_propagates_effectful_lambda_row() {
    CliTest::new(
        r#"
        pub unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000001) struct Wrap { v: Number }

        impl Wrap {
          fn apply<E!>(self, f: (Number) -> Number with E): Number with E {
            f(self.v)
          }
        }

        pub fn effectful_caller(): Number with core::system::Stdio {
          Wrap { v: 3 }.apply((n) => { core::system::Stdio::out!("hi"); n })
        }

        pub fn pure_caller(): Number {
          Wrap { v: 3 }.apply((n) => n + 1)
        }
        "#,
    )
    .check()
    .expect_success();
}

/// Negative: passing an effectful lambda through an `E!` inherent method into
/// a *pure* public caller is rejected — the concrete effect escapes undeclared.
#[test]
fn inherent_method_effect_escapes_pure_caller_rejected() {
    CliTest::new(
        r#"
        pub unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000002) struct Wrap { v: Number }

        impl Wrap {
          fn apply<E!>(self, f: (Number) -> Number with E): Number with E {
            f(self.v)
          }
        }

        pub fn bad(): Number {
          Wrap { v: 3 }.apply((n) => { core::system::Stdio::out!("leak"); n })
        }
        "#,
    )
    .check()
    .expect_error("uses ability `Stdio` but doesn't declare it");
}

/// A method-level `E!` chained *after* an impl-level type parameter. `List`'s
/// `impl<T> List<T>` supplies the impl-level `T`; the retyped `fold<A, E!>`
/// chains the method-level `A` and the row variable after it — impl-then-method
/// order, the same order the compiler allocates dictionary parameters in.
/// (Generic *user* struct inherent impls are a separate, pre-existing gap, so
/// `List` is the real vehicle for an impl-level type parameter here.)
#[test]
fn method_level_ability_var_chained_after_impl_type_param() {
    CliTest::new(
        r#"
        pub fn effectful(): Number with core::system::Stdio {
          [1, 2, 3].fold(0, (acc, n) => { core::system::Stdio::out!("step"); acc + n })
        }

        pub fn pure(): Number {
          [1, 2, 3].fold(0, (acc, n) => acc + n)
        }
        "#,
    )
    .check()
    .expect_success();
}

/// A bounded *and* effect-polymorphic method: `<U: Eq, E!>` — the `Eq`
/// dictionary parameter and the row variable coexist. The row variable carries
/// no bound so it never enters the dictionary list; the `U: Eq` dictionary is
/// allocated and used (`a.eq(b)`) exactly as without the `E!`.
#[test]
fn bounded_and_effect_polymorphic_inherent_method_coexist() {
    CliTest::new(
        r#"
        pub unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000004) struct Gate { v: Number }

        impl Gate {
          fn when_equal<U: core::cmp::Eq, E!>(
            self, a: U, b: U, g: () -> Number with E
          ): Number with E {
            if a.eq(b) { g() } else { 0 }
          }
        }

        pub fn effectful(): Number with core::system::Stdio {
          Gate { v: 0 }.when_equal(1, 1, () => { core::system::Stdio::out!("eq"); 5 })
        }

        pub fn pure(): Number {
          Gate { v: 0 }.when_equal(1, 2, () => 9)
        }
        "#,
    )
    .check()
    .expect_success();
}

/// `List::map` over an effectful lambda: the caller must declare the ability;
/// handling it at the call boundary discharges it; a pure map call site
/// requires nothing (the retyped core combinator instantiates `E := Empty`).
#[test]
fn list_map_with_effectful_lambda() {
    CliTest::new(
        r#"
        unique(AB000000-0000-0000-0000-00000000ABCD) ability Beep {
          fn beep(): Number { 0 }
        }

        // Declares the ability: an effectful mapping lambda propagates its row.
        pub fn declaring(): List<Number> with Beep {
          [1, 2, 3].map((n) => n + Beep::beep!())
        }

        // Handles the ability at the boundary: the enclosing function is pure.
        pub fn handling(): List<Number> {
          with { Beep::beep() => resume(10) } handle [1, 2, 3].map((n) => n + Beep::beep!())
        }

        // Pure lambda: `map`'s row instantiates to empty, so this stays pure.
        pub fn pure(): List<Number> {
          [1, 2, 3].map((n) => n * 2)
        }
        "#,
    )
    .check()
    .expect_success();
}

/// The retyped `Option`/`Result` combinators stay pure for pure callers:
/// `opt.map(pure)` and `res.map(pure)` require no abilities.
#[test]
fn option_and_result_map_stay_pure_for_pure_lambdas() {
    CliTest::new(
        r#"
        pub fn opt(): Option<Number> {
          Some(1).map((n) => n + 1)
        }

        pub fn res(): Result<Number, String> {
          Ok(1).map((n) => n + 1)
        }
        "#,
    )
    .check()
    .expect_success();
}

/// End-to-end: `ambient run` maps over a list with a lambda that performs
/// `Stdio`, declared on `run` (the platform supplies the default handler), and
/// the printed items reach stdout.
#[test]
fn run_list_map_performing_stdio() {
    CliTest::new(
        r#"
        pub fn run(): Number with core::system::Stdio {
          let doubled = [1, 2, 3].map((n) => {
            core::system::Stdio::out!(core::convert::to_string(n * 2));
            n * 2
          });
          doubled.length()
        }
        "#,
    )
    .expect_output("6");
}

/// `E!` on an impl *block* stays rejected: a block parameter parameterizes the
/// receiver type head, where an effect row cannot appear. The message points
/// at declaring `E!` on the method instead.
#[test]
fn ability_variable_on_impl_block_is_rejected() {
    CliTest::new(
        r#"
        pub unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000005) struct Box { v: Number }

        impl<E!> Box {
          fn run(self): Number { self.v }
        }
        "#,
    )
    .check()
    .expect_error("not supported on impl blocks");
}

/// `E!` on a trait method stays rejected — trait methods still discard all
/// method generics (a pre-existing gap), so the row variable would have
/// nothing to attach to.
#[test]
fn ability_variable_on_trait_method_is_rejected() {
    CliTest::new(
        r#"
        pub unique(AAAABBBB-CCCC-4DDD-8EEE-FFFF00000006) trait Run {
          fn run<E!>(self, body: () -> Number with E): Number;
        }
        "#,
    )
    .check()
    .expect_error("not yet supported on trait methods");
}

// ─────────────────────────────────────────────────────────────────────────────
// Canonical signature: spelling and rename stability (resolve level)
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve a single-module source's ability declarations against a core
/// registry (so the prelude primitive nominals its signatures hash against are
/// seeded), returning the resolved abilities.
fn resolve_source_abilities(
    src: &str,
) -> Vec<std::sync::Arc<ambient_engine::ability_resolver::DynAbility>> {
    let mut registry = ambient_engine::module_registry::ModuleRegistry::new();
    ambient_engine::core_library::register_core_modules(&mut registry, |s| {
        ambient_parser::parse(s).map_err(|e| e.to_string())
    })
    .expect("core modules must parse");

    let mut module = ambient_parser::parse(src).expect("source must parse");
    let (abilities, errors) =
        ambient_engine::infer::resolve_ability_declarations(&mut module, &registry);
    assert!(errors.is_empty(), "resolution errors: {errors:?}");
    abilities
}

/// The canonical signature of `run_it<T, E!>(body: () -> T with E): T` renders
/// its parameter as `fn() -> var0 with e0` and its return as `var0` — the row
/// variable numbered positionally, distinct from the type-variable counter.
#[test]
fn effect_polymorphic_method_canonical_spelling() {
    use ambient_core::SignatureHash;

    let abilities = resolve_source_abilities(
        r#"
        unique(AB000000-0000-0000-0000-0000000000FA) ability Runner {
          fn run_it<T, E!>(body: () -> T with E): T { body() }
        }
        "#,
    );
    let run_it = abilities[0].method("run_it").expect("run_it");
    assert_eq!(
        run_it.signature,
        SignatureHash::new(&["fn() -> var0 with e0"], "var0"),
        "an effect-polymorphic method renders its row variable as `e0`"
    );
}

/// A method's identity is independent of the row variable's *name*: two
/// same-shaped methods differing only in the spelling of `E!` produce the same
/// canonical signature (and therefore the same `MethodKey` input).
#[test]
fn effect_polymorphic_method_is_row_var_name_independent() {
    let e_named = resolve_source_abilities(
        r#"
        unique(AB000000-0000-0000-0000-0000000000FB) ability Runner {
          fn run_it<T, E!>(body: () -> T with E): T { body() }
        }
        "#,
    );
    let f_named = resolve_source_abilities(
        r#"
        unique(AB000000-0000-0000-0000-0000000000FC) ability Runner {
          fn run_it<T, F!>(body: () -> T with F): T { body() }
        }
        "#,
    );
    assert_eq!(
        e_named[0].method("run_it").unwrap().signature,
        f_named[0].method("run_it").unwrap().signature,
        "renaming the row variable must not move the method's signature hash"
    );
}

//! Integration tests for the REPL using the in-process test harness.
//!
//! These tests drive a real `ReplSession` directly (no PTY): each turn is a
//! synchronous `eval`/command, and all session and program output flows into
//! one buffer the assertions poll. They verify:
//! - Basic expression evaluation and definitions
//! - REPL commands (`:help`, `:clear`)
//! - Error handling and introspection
//! - The full pipeline (structs/enums/abilities/`use`, never semantics)
//! - The REPL as a deploy frontend (live upgrade, `:clear` drains, rejected
//!   deploys)

mod repl_harness;

use std::path::PathBuf;

use repl_harness::ReplTest;

/// Path to an example project shipped in the repo.
fn example_project(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

// ─────────────────────────────────────────────────────────────────────────────
// Basic Expression Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_basic_arithmetic() {
    ReplTest::new()
        .type_line("1 + 2")
        .expect_output("3")
        .shutdown();
}

#[test]
fn test_multiplication() {
    ReplTest::new()
        .type_line("6 * 7")
        .expect_output("42")
        .shutdown();
}

#[test]
fn test_boolean_literal() {
    ReplTest::new()
        .type_line("true")
        .expect_output("true")
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Definition Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_define_and_call_function() {
    ReplTest::new()
        .type_line("fn double(x) { x * 2 }")
        .expect_output("Defined: double")
        .type_line("double(21)")
        .expect_output("42")
        .shutdown();
}

#[test]
fn test_define_constant() {
    ReplTest::new()
        .type_line("const PI: Number = 3;")
        .expect_output("Defined: PI")
        .type_line("PI + 1")
        .expect_output("4")
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// REPL Command Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_help_command() {
    ReplTest::new()
        .type_line(":help")
        .expect_output("REPL Commands:")
        .expect_output(":quit")
        .shutdown();
}

#[test]
fn test_clear_command() {
    ReplTest::new()
        .type_line("const X: Number = 42;")
        .expect_output("Defined: X")
        .type_line(":clear")
        .expect_output("State cleared")
        .type_line("X")
        .expect_error("undefined") // X should be gone after clear
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Error Handling Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_parse_error() {
    ReplTest::new()
        .type_line("fn ()")
        .expect_error("error")
        .shutdown();
}

#[test]
fn test_undefined_variable() {
    ReplTest::new()
        .type_line("undefined_var")
        .expect_error("undefined")
        .shutdown();
}

#[test]
fn test_unterminated_string_does_not_crash() {
    // Bug: unterminated strings caused a panic instead of returning an error
    ReplTest::new()
        .type_line("\"s")
        .expect_error("unterminated") // Should show error, not crash
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Function Inspection Bug Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_core_free_function_inspects_as_function() {
    // Bug: Submitting `core::option::flatten` should inspect it as a function,
    // the same as if I printed the value of `fn example() {}<cr>example<cr>`.
    ReplTest::new()
        .type_line("core::option::flatten")
        // Should display as a function (like "fn flatten<T>(...): Option<T>").
        .expect_output("fn") // Functions should display with "fn" prefix
        .shutdown();
}

#[test]
fn test_dotted_module_path_is_rejected() {
    // Namespaces are addressed with `::`. A dotted path like `core.Number.sign`
    // is value/field access, not a namespace, so it must NOT resolve as a module
    // member — it parses as field access on the (undefined) value `core`.
    ReplTest::new()
        .type_line("core.Number.sign")
        .expect_error("undefined")
        .shutdown();
}

#[test]
fn test_user_defined_function_inspection() {
    // For comparison: user-defined functions should also be inspectable
    ReplTest::new()
        .type_line("fn example() { 42 }")
        .expect_output("Defined: example")
        .type_line("example")
        // Referencing a function by name should display it as a function
        .expect_output("fn") // Should show function representation
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Full-language Tests (unlocked by running the shared pipeline)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_define_struct_and_access_field() {
    // struct definitions are now supported; a value can be constructed and
    // its fields read back.
    ReplTest::new()
        .type_line("struct Point { x: Number, y: Number }")
        .expect_output("Defined: Point")
        .type_line("Point { x: 3, y: 4 }.x")
        .expect_output("3")
        .shutdown();
}

#[test]
fn test_define_enum() {
    ReplTest::new()
        .type_line("unique(A1B2C3D4-0000-0000-0000-000000000001) enum Color { Red, Green, Blue }")
        .expect_output("Defined: Color")
        .shutdown();
}

#[test]
fn test_define_type_alias() {
    ReplTest::new()
        .type_line("type Count = Number;")
        .expect_output("Defined: Count")
        .shutdown();
}

#[test]
fn test_cross_turn_use_and_ability_call() {
    // A `use` committed on one turn stays in scope for later turns, and the
    // full platform ability set is wired, so a bare `Stdio::out!` runs.
    ReplTest::new()
        .type_line("use core::system::Stdio;")
        .type_line("Stdio::out!(\"hello-repl\")")
        .expect_output("hello-repl")
        .shutdown();
}

#[test]
fn test_user_ability_declared_and_used_across_turns() {
    // A user `ability` declared on one turn stays in scope for later turns:
    // a function that performs it, then a handler that intercepts the perform,
    // all resolve against the committed `repl` module.
    ReplTest::new()
        .type_line(
            "unique(AB000000-0000-0000-0000-0000000000E1) ability Ping { fn ping(): Number { 7 } }",
        )
        .expect_output("Defined: Ping")
        .clear_output()
        .type_line("fn go(): Number with Ping { Ping::ping!() }")
        .expect_output("Defined: go")
        .clear_output()
        .type_line("with { Ping::ping() => resume(99) } handle go()")
        .expect_output("99")
        .shutdown();
}

#[test]
fn test_user_ability_default_impl_runs_unhandled() {
    // An unhandled perform of a user-declared ability runs the method's default
    // implementation — across turns and at the top level.
    ReplTest::new()
        .type_line(
            "unique(AB000000-0000-0000-0000-0000000000E2) ability Pong { fn pong(): Number { 5 } }",
        )
        .expect_output("Defined: Pong")
        .clear_output()
        .type_line("fn go2(): Number with Pong { Pong::pong!() }")
        .expect_output("Defined: go2")
        .clear_output()
        .type_line("go2()")
        .expect_output("5")
        .shutdown();
}

/// A one-file project whose `effects` module declares a `pub ability Ping`,
/// for driving the REPL against a project-defined user ability.
fn project_with_ping_ability(uuid: &str) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("ambient.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/effects.ab"),
        format!("pub unique({uuid}) ability Ping {{ fn ping(): Number {{ 7 }} }}\n"),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("src/main.ab"),
        "pub fn run(): Number { 0 }\n",
    )
    .unwrap();
    dir
}

#[test]
fn test_project_user_ability_imported_bare_in_repl() {
    // Started inside a project, the REPL can `use pkg::effects::Ping;` and then
    // perform/handle the imported user ability by its bare name.
    let dir = project_with_ping_ability("AB000000-0000-0000-0000-0000000000F1");
    ReplTest::with_project(dir.path())
        .type_line("use pkg::effects::Ping;")
        .clear_output()
        .type_line("with { Ping::ping() => resume(3) } handle Ping::ping!()")
        .expect_output("3")
        .shutdown();
}

#[test]
fn test_project_user_ability_fully_qualified_in_repl() {
    // The same project ability is reachable fully-qualified with no `use`.
    let dir = project_with_ping_ability("AB000000-0000-0000-0000-0000000000F2");
    ReplTest::with_project(dir.path())
        .type_line(
            "with { pkg::effects::Ping::ping() => resume(4) } handle pkg::effects::Ping::ping!()",
        )
        .expect_output("4")
        .shutdown();
}

#[test]
fn test_type_error_is_reported() {
    // A return-type mismatch is caught by the real type checker.
    ReplTest::new()
        .type_line("fn bad(): String { 42 }")
        .expect_error("String")
        .shutdown();
}

#[test]
fn test_redefine_function_across_turns() {
    // Redefinition replaces the earlier same-named definition (last wins).
    ReplTest::new()
        .type_line("fn f() { 1 }")
        .expect_output("Defined: f")
        .type_line("f()")
        .expect_output("1")
        .clear_output()
        .type_line("fn f() { 2 }")
        .expect_output("Defined: f")
        .type_line("f()")
        .expect_output("2")
        .shutdown();
}

#[test]
fn test_project_module_is_usable() {
    // Started inside a project, the REPL builds the whole package as its base
    // and project modules are reachable (fully-qualified or via `use`).
    ReplTest::with_project(&example_project("modules"))
        .type_line("pkg::numbers::gcd(48, 60)")
        .expect_output("12")
        .shutdown();
}

#[test]
fn test_repl_base_build_warms_off_a_prior_run_snapshot() {
    // The REPL base build is a read-only cache consumer: a prior `ambient run`
    // leaves a snapshot in the package store, and opening the REPL in that
    // project consumes it (warm base build) instead of recompiling cold. Seed
    // the snapshot with a real `ambient run`, then drive the in-process REPL
    // and confirm a project function resolves and evaluates through the
    // snapshot-backed base. (Warm-vs-cold byte identity is pinned by
    // `incremental_cache.rs`; this pins the REPL boundary wiring.)
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("ambient.toml"),
        "[package]\nname = \"warmrepl\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/util.ab"),
        "pub fn helper(): Number { 41 }\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("src/main.ab"),
        "use pkg::util::helper;\npub fn run(): Number { helper() + 1 }\n",
    )
    .unwrap();

    // Prior `ambient run`: builds the package and persists the snapshot the
    // REPL base build will warm off. Verify mode guards the run's own hits.
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_ambient"))
        .arg("run")
        .arg(dir.path())
        .env("AMBIENT_CACHE_VERIFY", "1")
        .output()
        .expect("spawn ambient run");
    assert!(
        status.status.success(),
        "prior `ambient run` failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert!(
        dir.path().join(".ambient/store").is_dir(),
        "the prior run must have created a package store"
    );

    ReplTest::with_project(dir.path())
        .type_line("pkg::util::helper() + 1")
        .expect_output("42")
        .shutdown();
}

#[test]
fn test_non_literal_const_is_rejected() {
    // Language parity: a `const` must be a literal. A computed value (here a
    // function call) is rejected — use a `fn` instead.
    ReplTest::new()
        .type_line("fn two() { 2 }")
        .expect_output("Defined: two")
        .type_line("const C: Number = two()")
        .expect_error("error")
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────
// Never (`!`) semantics through the REPL's own session/registry wiring
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_throw_in_value_position_in_repl_function() {
    // Bottom elimination through the REPL path: `Exception::throw!` returns
    // `!`, which must unify with the `Number` the other branch produces —
    // in a function defined on one turn and handled on a later one.
    ReplTest::new()
        .type_line(
            "fn clamp(v: Number): Number with Exception { if (v > 0) { v + 1 } else { Exception::throw!(\"low\") } }",
        )
        .expect_output("Defined: clamp")
        .clear_output()
        .type_line("with { Exception::throw(msg) => -1 } handle clamp(41)")
        .expect_output("42")
        .clear_output()
        .type_line("with { Exception::throw(msg) => -1 } handle clamp(-5)")
        .expect_output("-1")
        .shutdown();
}

#[test]
fn test_abstract_never_method_declared_and_performed_across_repl_turns() {
    // A REPL-declared ability with an abstract `: !` method: the declaration
    // commits without a default implementation, and its performs unwind to
    // the handler installed on a later line.
    ReplTest::new()
        .type_line(
            "unique(AB000000-0000-0000-0000-0000000000E5) ability Abort { fn abort(code: Number): !; }",
        )
        .expect_output("Defined: Abort")
        .clear_output()
        .type_line(
            "fn work(x: Number): Number with Abort { if (x > 10) { Abort::abort!(x) } else { x * 2 } }",
        )
        .expect_output("Defined: work")
        .clear_output()
        .type_line("with { Abort::abort(code) => code } handle work(50)")
        .expect_output("50")
        .clear_output()
        .type_line("with { Abort::abort(code) => code } handle work(3)")
        .expect_output("6")
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────
// The REPL as a deploy frontend (ref/live-upgrade.md, "Generations")
// ─────────────────────────────────────────────────────────────────────────

/// A ticker task body that prints `msg` then waits 50ms at an
/// interruptible perform (via the session-defined `mk_wait`). Fully
/// qualified performs keep the definition to one import; every turn the
/// tests type produces output to sync on.
///
/// The return is deliberately unannotated: an effectful function whose
/// return type no caller constrains once rendered a polymorphic `var0`
/// here but `unit` once the running task monomorphized it, so the deploy
/// core read the redefinition as a signature change and retired the name
/// instead of rebinding it — defeating the very live upgrade this test
/// exercises. The deploy signature now renders the declared interface (an
/// unannotated return is `var…` regardless of callers), so this rebinds.
fn ticker_body(msg: &str) -> String {
    format!(
        "fn tick() {{ core::system::Stdio::out!(\"{msg}\"); \
         core::system::Time::wait!(mk_wait()) }}"
    )
}

#[test]
fn test_redefining_a_task_body_live_upgrades_the_running_task() {
    // A definition turn is a deploy: redefining `tick` swaps the name
    // table, and the ticker task — which re-resolves its body's deployed
    // name (`repl::tick`) before every pass — picks the new code up on
    // its very next tick, with no re-ensure and no restart. The task also
    // proves incremental turns: it survives the turns between `ensure`
    // and the redefinition (the old declarative deploy would have drained
    // it as "no longer declared" on the next eval).
    ReplTest::new()
        .type_line("use core::time::Duration;")
        .type_line("fn mk_wait(): Duration { Duration::from_millis(50) }")
        .expect_output("Defined: mk_wait")
        .type_line(&ticker_body("tick one"))
        .expect_output("Defined: tick")
        .type_line("core::system::Task::ensure!(\"ticker\", tick)")
        .wait_for_output("tick one")
        .clear_output()
        .type_line(&ticker_body("tick two"))
        .expect_output("Defined: tick")
        .wait_for_output("tick two")
        .shutdown();
}

#[test]
fn test_rejected_deploy_is_a_turn_error_and_the_program_is_untouched() {
    // The State migration contract rejects pre-swap: committing a
    // definition whose statically-named `init_versioned` matches neither
    // the live cell's fingerprint (Number) on its old side (Bool) nor its
    // new side (String) errors the turn. Nothing was swapped and nothing
    // was committed: the cell keeps its value and the rejected name stays
    // undefined.
    ReplTest::new()
        .type_line("{ core::system::State::init!(\"counter\", () => 1); \"ready\" }")
        .expect_output("ready")
        .clear_output()
        .type_line(
            "fn setup() { core::system::State::init_versioned!(\"counter\", () => \"s\", (b: Bool) => \"s\") }",
        )
        .expect_error("deploy rejected")
        .expect_output("cell `counter`")
        .clear_output()
        .type_line("core::system::State::get!(\"counter\") + 1")
        .expect_output("2")
        .clear_output()
        .type_line("setup")
        .expect_error("undefined variable")
        .shutdown();
}

#[test]
fn test_clear_drains_running_tasks() {
    // `:clear` winds the running program down before rebuilding the
    // session: the ticker is drained (its Time::wait is interruptible)
    // and waited for, so it cannot keep printing into the fresh session.
    ReplTest::new()
        .type_line("use core::time::Duration;")
        .type_line("fn mk_wait(): Duration { Duration::from_millis(50) }")
        .expect_output("Defined: mk_wait")
        .type_line(&ticker_body("tick"))
        .expect_output("Defined: tick")
        .type_line("core::system::Task::ensure!(\"ticker\", tick)")
        .wait_for_output("tick")
        .type_line(":clear")
        .wait_for_output("task `ticker` drained")
        .wait_for_output("State cleared.")
        .clear_output()
        .type_line("1 + 1")
        .expect_output("2")
        .shutdown();
}

#[test]
fn test_resume_in_never_arm_is_rejected_in_repl() {
    // The dedicated catch-only diagnostic surfaces through the REPL: a
    // never-returning perform unwinds, so its arm cannot `resume`.
    ReplTest::new()
        .type_line(
            "unique(AB000000-0000-0000-0000-0000000000E6) ability Halt { fn halt(code: Number): !; }",
        )
        .expect_output("Defined: Halt")
        .clear_output()
        .type_line("fn go3(): Number with Halt { Halt::halt!(1) }")
        .expect_output("Defined: go3")
        .clear_output()
        .type_line("with { Halt::halt(code) => resume(code) } handle go3()")
        .expect_error("cannot `resume` `Halt::halt`")
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait Bounds Across Turns
// ─────────────────────────────────────────────────────────────────────────────
//
// The REPL re-checks and recompiles the whole committed module every turn, so
// the trait registry, scheme bounds, and dictionaries are rebuilt from source
// each time. These tests pin that a bound declared on one turn still solves
// (and threads its dictionary) when exercised on a later turn.

#[test]
fn test_bounded_fn_declared_then_called_across_turns() {
    // A bounded generic committed on one turn is called on a later turn: the
    // `<` operator dispatches through the reserved `Ord` dictionary, and the
    // concrete `Number` argument supplies the impl at the (later) call site.
    ReplTest::new()
        .type_line("fn min_of<T: Ord>(a: T, b: T): T { if a < b { a } else { b } }")
        .expect_output("Defined: min_of")
        .clear_output()
        .type_line("min_of(7, 3)")
        .expect_output("3")
        .shutdown();
}

#[test]
fn test_user_trait_impl_and_bound_across_turns() {
    // A user trait, a struct, its impl, and a function bounded by that trait
    // are each declared on separate turns; a final turn calls the bounded
    // function on the struct. The dictionary is built from the impl committed
    // two turns earlier — the cross-turn re-registration path for a
    // *user-declared* (non-reserved) trait.
    ReplTest::new()
        .type_line(
            "unique(BC000000-0000-0000-0000-000000000701) trait Weight { fn grams(self): Number; }",
        )
        .expect_output("Defined: Weight")
        .clear_output()
        .type_line("unique(BC000000-0000-0000-0000-000000000702) struct Brick { kg: Number }")
        .expect_output("Defined: Brick")
        .clear_output()
        .type_line("impl Weight for Brick { fn grams(self): Number { self.kg * 1000 } }")
        .clear_output()
        .type_line("fn heft<T: Weight>(x: T): Number { x.grams() }")
        .expect_output("Defined: heft")
        .clear_output()
        .type_line("heft(Brick { kg: 2 })")
        .expect_output("2000")
        .shutdown();
}

#[test]
fn test_list_contains_uses_eq_bound_in_repl() {
    // `List::contains` is bounded by `T: Eq`; calling it in the REPL builds
    // the Number Eq dictionary at the call site like any other bounded call.
    ReplTest::new()
        .type_line("[1, 2, 3].contains(2)")
        .expect_output("true")
        .clear_output()
        .type_line("[1, 2, 3].contains(9)")
        .expect_output("false")
        .shutdown();
}

#[test]
fn test_bounded_fn_over_user_impl_across_turns() {
    // A reserved-trait (`Eq`) impl for a user struct declared on one turn,
    // then consumed by a bounded generic on later turns: the same-named-item
    // commit path keeps the impl's dictionary reachable.
    ReplTest::new()
        .type_line("unique(BC000000-0000-0000-0000-000000000703) struct Tag { id: Number }")
        .expect_output("Defined: Tag")
        .clear_output()
        .type_line("impl Eq for Tag { fn eq(self, other: Tag): Bool { self.id == other.id } }")
        .clear_output()
        .type_line("fn same<T: Eq>(a: T, b: T): Bool { a.eq(b) }")
        .expect_output("Defined: same")
        .clear_output()
        .type_line("same(Tag { id: 5 }, Tag { id: 5 })")
        .expect_output("true")
        .shutdown();
}

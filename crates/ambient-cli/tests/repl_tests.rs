//! Integration tests for the REPL using the in-process test harness.
//!
//! These tests drive a real `ReplSession` directly (no PTY): each turn is a
//! synchronous `eval`/command, and all session and program output flows into
//! one buffer the assertions poll. They verify:
//! - Expression evaluation, `use` imports, and session bindings (`x = expr`)
//! - Definitions are rejected (the REPL explores code; files author it)
//! - Scope anchoring: `pkg`/`self`/`super` resolve like a file at the
//!   launch directory, with an informative note outside a project
//! - REPL commands (`:help`, `:clear`, `:reload`) and live reload from disk
//! - Error rendering (the synthetic entry never leaks) and introspection
//! - The REPL as a deploy frontend (tasks keep running, rejected deploys)

mod repl_harness;

use std::path::PathBuf;

use repl_harness::ReplTest;

/// Path to an example project shipped in the repo.
fn example_project(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

/// Write a minimal project: an `ambient.toml` plus the given
/// `src/`-relative files.
fn project_with(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("ambient.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    for (rel, source) in files {
        let path = dir.path().join("src").join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    dir
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
fn test_boolean_literal() {
    ReplTest::new()
        .type_line("true")
        .expect_output("true")
        .shutdown();
}

#[test]
fn test_list_contains_uses_eq_bound() {
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
fn test_exception_handled_in_expression() {
    // Never semantics at the prompt: `Exception::throw!` returns `!`, which
    // unifies with the `Number` of the other branch, and the handler's arm
    // cannot resume (catch-only) — all in a single expression turn.
    ReplTest::new()
        .type_line(
            "with { Exception::throw(msg) => -1 } handle { if (1 > 0) { Exception::throw!(\"low\") } else { 7 } }",
        )
        .expect_output("-1")
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Session Bindings (`x = expr`)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_binding_saves_and_reuses_value() {
    ReplTest::new()
        .type_line("x = 40 + 2")
        .expect_output("x: Number")
        .expect_output("42")
        .clear_output()
        .type_line("x * 2")
        .expect_output("84")
        .shutdown();
}

#[test]
fn test_binding_rebinds_and_changes_type() {
    // Bindings are mutable session state: rebinding replaces the value, and
    // the new value may have a completely different type.
    ReplTest::new()
        .type_line("x = 1")
        .type_line("x = x + 1")
        .clear_output()
        .type_line("x")
        .expect_output("2")
        .clear_output()
        .type_line("x = \"now a string\"")
        .expect_output("x: String")
        .clear_output()
        .type_line("x.length()")
        .expect_output("12")
        .shutdown();
}

#[test]
fn test_binding_initializer_runs_once() {
    // The initializer's effects run exactly once, at the binding turn —
    // later uses pass the saved value, they do not re-evaluate.
    ReplTest::new()
        .type_line("x = { core::system::Stdio::out!(\"computed\"); 5 }")
        .expect_output("computed")
        .clear_output()
        .type_line("x + x")
        .expect_output("10")
        .shutdown();

    // Exactly once: the second turn must not have printed again. (The
    // builder is consumed by shutdown, so assert via a fresh session.)
    let repl = ReplTest::new()
        .type_line("x = { core::system::Stdio::out!(\"computed\"); 5 }")
        .clear_output()
        .type_line("x + x")
        .expect_output("10");
    assert!(
        !repl.output().contains("computed"),
        "the initializer must not re-run on later turns:\n{}",
        repl.output()
    );
    repl.shutdown();
}

#[test]
fn test_binding_to_closure_is_callable() {
    ReplTest::new()
        .type_line("f = (x) => x + 1")
        .clear_output()
        .type_line("f(41)")
        .expect_output("42")
        .shutdown();
}

#[test]
fn test_binding_with_undetermined_type_is_rejected() {
    // `[]` is `List<?>` — nothing determines the element type, so saving it
    // would let a later use dictate a type the value doesn't have.
    ReplTest::new()
        .type_line("xs = []")
        .expect_error("not fully determined")
        .clear_output()
        .type_line("xs = [1, 2, 3]")
        .expect_output("xs: List<Number>")
        .clear_output()
        .type_line("xs.contains(2)")
        .expect_output("true")
        .shutdown();
}

#[test]
fn test_binding_shadows_module_name() {
    // Locals shadow module-level names (ref/modules.md): a binding named
    // like a core module makes the bare name a value, not a namespace.
    ReplTest::new()
        .type_line("option = 5")
        .clear_output()
        .type_line("option + 1")
        .expect_output("6")
        .shutdown();
}

#[test]
fn test_equality_is_not_a_binding() {
    ReplTest::new()
        .type_line("x = 1")
        .clear_output()
        .type_line("x == 1")
        .expect_output("true")
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Definitions Are Rejected
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_definitions_are_rejected_with_guidance() {
    let cases = [
        ("fn double(x) { x * 2 }", "a function"),
        ("const PI: Number = 3;", "a constant"),
        ("struct Point { x: Number, y: Number }", "a struct"),
        ("type Count = Number;", "a type alias"),
        (
            "unique(A1B2C3D4-0000-0000-0000-000000000001) enum Color { Red, Green }",
            "an enum",
        ),
        (
            "unique(AB000000-0000-0000-0000-0000000000E1) ability Ping { fn ping(): Number { 7 } }",
            "an ability",
        ),
        (
            "unique(BC000000-0000-0000-0000-000000000701) trait Weight { fn grams(self): Number; }",
            "a trait",
        ),
        (
            "impl Eq for Tag { fn eq(self, other: Tag): Bool { true } }",
            "an impl",
        ),
    ];
    for (line, what) in cases {
        let repl = ReplTest::new()
            .type_line(line)
            .expect_error("the REPL doesn't host definitions");
        assert!(
            repl.output().contains(what),
            "rejection for `{line}` should name {what}:\n{}",
            repl.output()
        );
        repl.shutdown();
    }
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
fn test_clear_command_drops_bindings_and_imports() {
    ReplTest::new()
        .type_line("x = 42")
        .type_line(":clear")
        .expect_output("State cleared")
        .clear_output()
        .type_line("x")
        .expect_error("undefined")
        .shutdown();
}

#[test]
fn test_reload_outside_project_is_a_note() {
    ReplTest::new()
        .type_line(":reload")
        .expect_output("not in a project")
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
    ReplTest::new()
        .type_line("\"s")
        .expect_error("unterminated")
        .shutdown();
}

#[test]
fn test_type_error_is_reported() {
    ReplTest::new()
        .type_line("\"a\" + 1")
        .expect_error("error")
        .shutdown();
}

#[test]
fn test_synthetic_entry_never_leaks_into_errors() {
    // The turn's wrapper function is an implementation detail: no rendered
    // diagnostic — type error, runtime fault, deploy note — may name it.
    for line in ["\"a\" + 1", "undefined_var", "[1].contains(\"x\")"] {
        let repl = ReplTest::new().type_line(line).expect_error("error");
        assert!(
            !repl.output().contains("__repl_entry"),
            "`{line}` leaked the synthetic entry:\n{}",
            repl.output()
        );
        repl.shutdown();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Imports
// ─────────────────────────────────────────────────────────────────────────────

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
fn test_failed_import_is_an_error() {
    ReplTest::new()
        .type_line("use core::no_such_module::whatever;")
        .expect_error("error")
        .shutdown();
}

#[test]
fn test_reimport_replaces_earlier_import() {
    // Re-importing a name (here, under an alias that collides) is
    // last-wins, like redefinition used to be — no duplicate-name error.
    ReplTest::new()
        .type_line("use core::system::Stdio;")
        .type_line("use core::system::Log as Stdio;")
        .clear_output()
        .type_line("Stdio::info!(\"via-log\")")
        .expect_output("via-log")
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// Scope Anchoring (pkg / self / super)
// ─────────────────────────────────────────────────────────────────────────────

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
fn test_self_and_super_resolve_at_the_launch_directory() {
    // The session's scope is a virtual file in the launch directory:
    // started in `src/net`, `self::client` is `src/net/client.ab` and
    // `super::util` is `src/util.ab` — exactly as in a file authored there.
    let dir = project_with(&[
        ("main.ab", "pub fn run(): Number { 0 }\n"),
        ("util.ab", "pub fn root_helper(): Number { 10 }\n"),
        ("net/client.ab", "pub fn port(): Number { 8080 }\n"),
    ]);
    ReplTest::with_project(&dir.path().join("src/net"))
        .type_line("self::client::port()")
        .expect_output("8080")
        .clear_output()
        .type_line("super::util::root_helper()")
        .expect_output("10")
        .clear_output()
        .type_line("use self::client;")
        .type_line("client::port() + 1")
        .expect_output("8081")
        .shutdown();
}

#[test]
fn test_project_roots_error_informatively_outside_a_project() {
    let repl = ReplTest::new()
        .type_line("use pkg::utils;")
        .expect_error("outside an ambient package");
    repl.shutdown();

    ReplTest::new()
        .type_line("self::helper()")
        .expect_error("outside an ambient package")
        .shutdown();
}

#[test]
fn test_project_user_ability_imported_bare_in_repl() {
    // Started inside a project, the REPL can `use pkg::effects::Ping;` and then
    // perform/handle the imported user ability by its bare name.
    let dir = project_with(&[
        ("main.ab", "pub fn run(): Number { 0 }\n"),
        (
            "effects.ab",
            "pub unique(AB000000-0000-0000-0000-0000000000F1) ability Ping { fn ping(): Number { 7 } }\n",
        ),
    ]);
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
    let dir = project_with(&[
        ("main.ab", "pub fn run(): Number { 0 }\n"),
        (
            "effects.ab",
            "pub unique(AB000000-0000-0000-0000-0000000000F2) ability Ping { fn ping(): Number { 7 } }\n",
        ),
    ]);
    ReplTest::with_project(dir.path())
        .type_line(
            "with { pkg::effects::Ping::ping() => resume(4) } handle pkg::effects::Ping::ping!()",
        )
        .expect_output("4")
        .shutdown();
}

#[test]
fn test_binding_holds_project_struct_across_turns() {
    // A project struct value saved as a binding is usable on later turns —
    // methods dispatch on the saved value.
    let dir = project_with(&[
        ("main.ab", "pub fn run(): Number { 0 }\n"),
        (
            "geometry.ab",
            "pub unique(BC000000-0000-0000-0000-000000000801) struct Point { x: Number, y: Number }\n\
             impl Point { fn sum(self): Number { self.x + self.y } }\n",
        ),
    ]);
    ReplTest::with_project(dir.path())
        .type_line("use pkg::geometry::Point;")
        .type_line("p = Point { x: 3, y: 4 }")
        .clear_output()
        .type_line("p.sum()")
        .expect_output("7")
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
    let dir = project_with(&[
        ("util.ab", "pub fn helper(): Number { 41 }\n"),
        (
            "main.ab",
            "use pkg::util::helper;\npub fn run(): Number { helper() + 1 }\n",
        ),
    ]);

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

// ─────────────────────────────────────────────────────────────────────────────
// Inspection
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_core_free_function_inspects_as_function() {
    ReplTest::new()
        .type_line("core::option::flatten")
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

// ─────────────────────────────────────────────────────────────────────────────
// Live Reload (file edits refresh the session)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_reload_picks_up_edited_project_code() {
    let dir = project_with(&[
        ("main.ab", "pub fn run(): Number { 0 }\n"),
        ("answers.ab", "pub fn answer(): Number { 1 }\n"),
    ]);
    let repl = ReplTest::with_project(dir.path())
        .type_line("pkg::answers::answer()")
        .expect_output("1")
        .clear_output();

    std::fs::write(
        dir.path().join("src/answers.ab"),
        "pub fn answer(): Number { 2 }\n",
    )
    .unwrap();

    repl.type_line(":reload")
        .expect_output("Reloaded.")
        .clear_output()
        .type_line("pkg::answers::answer()")
        .expect_output("2")
        .shutdown();
}

#[test]
fn test_reload_keeps_bindings_and_reports_broken_builds() {
    let dir = project_with(&[
        ("main.ab", "pub fn run(): Number { 0 }\n"),
        ("answers.ab", "pub fn answer(): Number { 1 }\n"),
    ]);
    let repl = ReplTest::with_project(dir.path())
        .type_line("x = pkg::answers::answer() + 10")
        .clear_output();

    // A broken edit reports and leaves the session usable on the old base.
    std::fs::write(
        dir.path().join("src/answers.ab"),
        "pub fn answer(): Number { \"broken\" }\n",
    )
    .unwrap();
    let repl = repl
        .type_line(":reload")
        .expect_error("error")
        .clear_output()
        .type_line("x")
        .expect_output("11");

    // Fixing the file reloads cleanly; the binding's saved value persists.
    std::fs::write(
        dir.path().join("src/answers.ab"),
        "pub fn answer(): Number { 5 }\n",
    )
    .unwrap();
    repl.type_line(":reload")
        .expect_output("Reloaded.")
        .clear_output()
        .type_line("x + pkg::answers::answer()")
        .expect_output("16")
        .shutdown();
}

#[test]
fn test_reload_live_upgrades_a_running_task() {
    // The inversion of the old REPL's live editing: the task is defined in a
    // project file, ensured from the prompt, and picks up an edit to its
    // body on the next pass after `:reload` — no re-ensure, no restart.
    let ticker = |msg: &str| {
        format!(
            "use core::time::Duration;\n\
             use core::system::{{Stdio, Time}};\n\
             pub fn tick(): () with Stdio, Time {{ Stdio::out!(\"{msg}\"); \
             Time::wait!(Duration::from_millis(50)) }}\n"
        )
    };
    let dir = project_with(&[
        ("main.ab", "pub fn run(): Number { 0 }\n"),
        ("ticker.ab", &ticker("tick one")),
    ]);
    let repl = ReplTest::with_project(dir.path())
        .type_line("core::system::Task::ensure!(\"ticker\", pkg::ticker::tick)")
        .wait_for_output("tick one")
        .clear_output();

    std::fs::write(dir.path().join("src/ticker.ab"), ticker("tick two")).unwrap();

    repl.type_line(":reload")
        .expect_output("Reloaded.")
        .wait_for_output("tick two")
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────
// The REPL as a deploy frontend (ref/live-upgrade.md, "Generations")
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_rejected_deploy_is_a_turn_error_and_the_program_is_untouched() {
    // The State migration contract rejects pre-swap: a turn whose
    // statically-named `init_versioned` matches neither the live cell's
    // fingerprint (Number) on its old side (Bool) nor its new side (String)
    // errors the turn. Nothing was swapped: the cell keeps its value.
    ReplTest::new()
        .type_line("{ core::system::State::init!(\"counter\", () => 1); \"ready\" }")
        .expect_output("ready")
        .clear_output()
        .type_line(
            "core::system::State::init_versioned!(\"counter\", () => \"s\", (b: Bool) => \"s\")",
        )
        .expect_error("deploy rejected")
        .expect_output("cell `counter`")
        .clear_output()
        .type_line("core::system::State::get!(\"counter\") + 1")
        .expect_output("2")
        .shutdown();
}

#[test]
fn test_clear_drains_running_tasks() {
    // `:clear` winds the running program down before rebuilding the
    // session: the ticker is drained (its Time::wait is interruptible)
    // and waited for, so it cannot keep printing into the fresh session.
    let dir = project_with(&[
        ("main.ab", "pub fn run(): Number { 0 }\n"),
        (
            "ticker.ab",
            "use core::time::Duration;\n\
             use core::system::{Stdio, Time};\n\
             pub fn tick(): () with Stdio, Time { Stdio::out!(\"tick\"); \
             Time::wait!(Duration::from_millis(50)) }\n",
        ),
    ]);
    ReplTest::with_project(dir.path())
        .type_line("core::system::Task::ensure!(\"ticker\", pkg::ticker::tick)")
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
fn test_module_listing_shows_pub_surface_and_children() {
    // A module lists its `pub` exports (functions with signatures) and its
    // child modules; private items don't show — inspection answers "what
    // can I call from here".
    let dir = project_with(&[
        ("main.ab", "pub fn run(): Number { 0 }\n"),
        (
            "util.ab",
            "/// Adds one.\npub fn add_one(n: Number): Number { n + 1 }\n\
             fn hidden(): Number { 0 }\n",
        ),
        ("util/extra.ab", "pub fn deep(): Number { 1 }\n"),
    ]);
    let repl = ReplTest::with_project(dir.path())
        .type_line("pkg::util")
        .expect_output("module pkg::util")
        .expect_output("add_one")
        .expect_output("mod extra");
    assert!(
        !repl.output().contains("hidden"),
        "private items must not list:\n{}",
        repl.output()
    );
    repl.shutdown();
}

#[test]
fn test_pkg_and_self_list_modules() {
    let dir = project_with(&[
        ("main.ab", "pub fn run(): Number { 0 }\n"),
        ("util.ab", "pub fn helper(): Number { 1 }\n"),
        ("net/client.ab", "pub fn port(): Number { 8080 }\n"),
    ]);
    // `pkg` lists the package root's own surface plus its top-level
    // modules (and not `core`). The root module is the package itself —
    // there is no `mod main` entry.
    let repl = ReplTest::with_project(dir.path())
        .type_line("pkg")
        .expect_output("module pkg")
        .expect_output("fn run")
        .expect_output("mod net")
        .expect_output("mod util");
    assert!(
        !repl.output().contains("mod core"),
        "`pkg` must not list core as a package member:\n{}",
        repl.output()
    );
    repl.shutdown();

    // `self` at src/net is the net directory: it lists `client`.
    ReplTest::with_project(&dir.path().join("src/net"))
        .type_line("self")
        .expect_output("mod client")
        .shutdown();
}

#[test]
fn test_member_inspection_shows_signature_and_doc() {
    let dir = project_with(&[
        ("main.ab", "pub fn run(): Number { 0 }\n"),
        (
            "util.ab",
            "/// Adds one to a number.\npub fn add_one(n: Number): Number { n + 1 }\n",
        ),
    ]);
    ReplTest::with_project(dir.path())
        .type_line("pkg::util::add_one")
        .expect_output("fn add_one(n: Number): Number")
        .expect_output("Adds one to a number.")
        .shutdown();
}

#[test]
fn test_sig_command_shows_signatures_and_binding_types() {
    let dir = project_with(&[
        ("main.ab", "pub fn run(): Number { 0 }\n"),
        (
            "util.ab",
            "/// Doubles.\npub fn double(n: Number): Number { n * 2 }\n",
        ),
    ]);
    ReplTest::with_project(dir.path())
        .type_line(":sig pkg::util::double")
        .expect_output("fn double(n: Number): Number")
        .clear_output()
        .type_line("x = 5")
        .clear_output()
        .type_line(":sig x")
        .expect_output("x: Number")
        .shutdown();
}

#[test]
fn test_type_command_infers_without_running() {
    ReplTest::new()
        .type_line(":type 1 + 2")
        .expect_output("Number")
        .clear_output()
        .type_line(":type { core::system::Stdio::out!(\"nope\"); [true] }")
        .expect_output("List<Bool>")
        .shutdown();

    // The expression was checked, never run.
    let repl = ReplTest::new().type_line(":type { core::system::Stdio::out!(\"nope\"); [true] }");
    assert!(
        !repl.output().contains("nope"),
        ":type must not execute the expression:\n{}",
        repl.output()
    );
    repl.shutdown();
}

#[test]
fn test_private_member_inspection_reads_as_absent() {
    let dir = project_with(&[
        ("main.ab", "pub fn run(): Number { 0 }\n"),
        ("util.ab", "fn hidden(): Number { 0 }\n"),
    ]);
    ReplTest::with_project(dir.path())
        .type_line("pkg::util::hidden")
        .expect_error("error")
        .shutdown();
}

#[test]
fn test_reexported_ability_inspects_with_methods() {
    // `core::system::Stdio` is a `pub use` re-export: inspection chases the
    // chain to the defining module and shows the ability's methods and doc.
    ReplTest::new()
        .type_line("core::system::Stdio")
        .expect_output("ability Stdio")
        .expect_output("fn out(message: String): ()")
        .expect_output("core::system::stdio::Stdio")
        .shutdown();
}

#[test]
fn test_imported_bare_name_inspects() {
    // After `use`, the bare imported name inspects as its target.
    ReplTest::new()
        .type_line("use core::system::Stdio;")
        .clear_output()
        .type_line("Stdio")
        .expect_output("ability Stdio")
        .shutdown();
}

#[test]
fn test_prelude_names_inspect_bare() {
    // Prelude re-exports (`Option`, `Number`) inspect without any import.
    ReplTest::new()
        .type_line("Option")
        .expect_output("enum Option<T>")
        .expect_output("Some(T)")
        .shutdown();
}

#[test]
fn test_core_module_listing_has_no_duplicates() {
    let repl = ReplTest::new()
        .type_line("core")
        .expect_output("module core");
    let output = repl.output();
    let count = output.matches("mod option;").count();
    assert_eq!(count, 1, "each child must list once:\n{output}");
    repl.shutdown();
}

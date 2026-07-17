//! Tests for the incremental analysis session (`session.rs`).
//!
//! Split out via `#[path]` to keep `session.rs` within its line budget.

use super::*;
use std::fs;
use tempfile::TempDir;

fn write_package(files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path();
    fs::write(
        root.join("ambient.toml"),
        "[package]\nname = \"test\"\nversion = \"0.1.0\"\n\n[build]\nsrc = \"src\"\n",
    )
    .expect("manifest");
    let src = root.join("src");
    fs::create_dir_all(&src).expect("src");
    for (name, content) in files {
        fs::write(src.join(name), content).expect("module");
    }
    dir
}

fn open(dir: &TempDir) -> AnalysisSession {
    let package = AnalysisPackage::open(dir.path()).expect("open package");
    AnalysisSession::new(package)
}

fn module_path(name: &str) -> ModulePath {
    // Module paths are mounted under the package name (`test`); the root
    // `main.ab` is the mount itself.
    ModulePath::from_relative_file_path(std::path::Path::new(&format!("test/{name}.ab")))
        .expect("module path")
}

/// A stable, comparable view of a module's diagnostics.
fn diags(result: &AnalysisResult) -> Vec<(u32, u32, String)> {
    result
        .diagnostics()
        .into_iter()
        .map(|d| (d.span.start, d.span.end, d.message))
        .collect()
}

/// The warm result must equal a fresh cold analysis of the session's
/// *current* (in-memory, possibly edited) sources — the correctness bar for
/// the whole phase. `session.package().analyze_all()` is the cold path over
/// exactly those sources, sharing none of the session's memo state.
fn assert_matches_cold(session: &AnalysisSession, warm: &HashMap<String, AnalysisResult>) {
    let cold = session.package().analyze_all();
    let mut keys: Vec<_> = cold.keys().cloned().collect();
    keys.sort();
    for key in keys {
        assert_eq!(
            diags(&warm[&key]),
            diags(&cold[&key]),
            "warm vs cold diagnostics differ for `{key}`"
        );
    }
}

#[test]
fn verify_oracle_passes_on_warm_hits() {
    // The `AMBIENT_ANALYSIS_VERIFY` oracle: with verify on, every memo hit
    // is recomputed cold and asserted byte-identical. Set the field
    // directly (env is process-global — unsafe to mutate under parallel
    // tests) and drive a body-only edit so the unedited module hits under
    // verify. A clean pass proves the hit path agrees with a fresh check;
    // a stale hit would panic inside `serve`.
    let dir = write_package(&[
        (
            "main.ab",
            "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
        ),
        ("utils.ab", "pub fn helper(): Number { 1 }\n"),
    ]);
    let mut session = open(&dir);
    let _ = session.analyze_all();
    session.verify = true;

    // Edit `main`'s body only; `utils` is untouched and hits under verify.
    session.edit_module(
        &module_path("main"),
        "use pkg::utils::helper;\npub fn run(): Number { helper() + 0 }\n".to_string(),
    );
    let warm = session.analyze_all();
    assert_matches_cold(&session, &warm);
}

#[test]
fn cold_pass_checks_every_module_once() {
    let dir = write_package(&[
        (
            "main.ab",
            "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
        ),
        ("utils.ab", "pub fn helper(): Number { 1 }\n"),
    ]);
    let mut session = open(&dir);
    let results = session.analyze_all();
    assert_eq!(session.rechecks(), 2, "cold: both modules checked");
    for (key, r) in &results {
        assert!(r.diagnostics().is_empty(), "{key}: {:?}", diags(r));
    }
}

#[test]
fn body_only_edit_rechecks_exactly_one_module() {
    let dir = write_package(&[
        (
            "main.ab",
            "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
        ),
        ("utils.ab", "pub fn helper(): Number { 1 }\n"),
    ]);
    let mut session = open(&dir);
    let _ = session.analyze_all();
    let base = session.rechecks();

    // Edit `main`'s body only — its signature (interface) is unchanged.
    session.edit_module(
        &module_path("main"),
        "use pkg::utils::helper;\npub fn run(): Number { helper() + 0 }\n".to_string(),
    );
    let warm = session.analyze_all();
    assert_eq!(
        session.rechecks() - base,
        1,
        "only the edited module re-checks"
    );
    assert_matches_cold(&session, &warm);
}

#[test]
fn interface_edit_rechecks_changed_module_and_dependents() {
    let dir = write_package(&[
        (
            "main.ab",
            "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
        ),
        ("utils.ab", "pub fn helper(): Number { 1 }\n"),
        ("other.ab", "pub fn unrelated(): Number { 2 }\n"),
    ]);
    let mut session = open(&dir);
    let _ = session.analyze_all();
    let base = session.rechecks();

    // Change `helper`'s signature: `main` depends on it and must re-check;
    // `other` does not and must stay memoized. (Not an impl/ability edit,
    // so the dispatch surface is untouched and `other`'s key holds.)
    session.edit_module(
        &module_path("utils"),
        "pub fn helper(): String { \"x\" }\n".to_string(),
    );
    let warm = session.analyze_all();
    assert_eq!(
        session.rechecks() - base,
        2,
        "changed module + its one dependent re-check, `other` stays memoized"
    );
    assert_matches_cold(&session, &warm);
}

#[test]
fn dependency_body_only_edit_does_not_recheck_dependents() {
    // Phase 5 step 3 (analysis side, already satisfied): a *dependency's*
    // function-body edit changes neither its signature nor any other
    // check-level input a dependent observes — a plain function body is
    // absent from the interface hash — so the dependent's memo key holds
    // and it is *not* re-checked. Only the edited module re-checks. (The
    // build cache, by contrast, must still relink the dependent's objects
    // through link validation, since a body edit moves the callee's final
    // content hash; skipping *its* redundant re-check is the remaining,
    // unlanded, build-cache half of step 3.)
    let dir = write_package(&[
        (
            "main.ab",
            "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
        ),
        ("utils.ab", "pub fn helper(): Number { 1 }\n"),
    ]);
    let mut session = open(&dir);
    let _ = session.analyze_all();
    let base = session.rechecks();

    // Edit only `utils`'s body; its signature (interface) is unchanged.
    session.edit_module(
        &module_path("utils"),
        "pub fn helper(): Number { 2 }\n".to_string(),
    );
    let warm = session.analyze_all();
    assert_eq!(
        session.rechecks() - base,
        1,
        "only the edited dependency re-checks; the dependent's memo holds"
    );
    assert_matches_cold(&session, &warm);
}

#[test]
fn dispatch_surface_edit_flows_through_every_key() {
    // An impl edit moves the changed module's interface (impls are in it)
    // and the build-global dispatch surface, so it is an interface-class
    // change; the result must still match a cold analysis.
    let dir = write_package(&[
        (
            "shapes.ab",
            "unique(A1B2C3D4-0000-0000-0000-000000000001) struct P { x: Number }\n\
             impl P {\n    fn get(self): Number { self.x }\n}\n",
        ),
        (
            "main.ab",
            "use pkg::shapes::P;\npub fn run(): Number { P { x: 1 }.get() }\n",
        ),
    ]);
    let mut session = open(&dir);
    let cold = session.analyze_all();
    assert_matches_cold(&session, &cold);

    session.edit_module(
        &module_path("shapes"),
        "unique(A1B2C3D4-0000-0000-0000-000000000001) struct P { x: Number }\n\
         impl P {\n    fn get(self): Number { self.x + 0 }\n}\n"
            .to_string(),
    );
    let warm = session.analyze_all();
    assert_matches_cold(&session, &warm);
}

#[test]
fn unrelated_impl_body_edit_leaves_unrelated_modules_cached() {
    // Phase 5 step 2: the dispatch surface is body-free. An impl *body*
    // edit in `shapes` re-checks `shapes` (its own source) and its
    // dependent `consumer` (the full interface hash still retains bodies,
    // a spurious-but-safe re-check through the dependency channel), but
    // leaves `unrelated` — which names neither the type nor the impl's
    // module — memoized. Under the old body-bearing *global* dispatch
    // hash the edit moved every module's key, so all three re-checked.
    let dir = write_package(&[
        (
            "shapes.ab",
            "pub unique(A1B2C3D4-0000-0000-0000-000000000042) struct P { x: Number }\n\
             impl P {\n    fn get(self): Number { self.x }\n}\n",
        ),
        (
            "consumer.ab",
            "use pkg::shapes::P;\npub fn run(): Number { P { x: 1 }.get() }\n",
        ),
        ("unrelated.ab", "pub fn f(): Number { 2 }\n"),
    ]);
    let mut session = open(&dir);
    let _ = session.analyze_all();
    let base = session.rechecks();

    session.edit_module(
        &module_path("shapes"),
        "pub unique(A1B2C3D4-0000-0000-0000-000000000042) struct P { x: Number }\n\
         impl P {\n    fn get(self): Number { self.x + 0 }\n}\n"
            .to_string(),
    );
    let warm = session.analyze_all();
    assert_eq!(
        session.rechecks() - base,
        2,
        "`shapes` (source) + `consumer` (dep) re-check; `unrelated` stays memoized"
    );
    assert_matches_cold(&session, &warm);
}

#[test]
fn impl_add_on_a_package_type_leaves_unrelated_modules_memoized() {
    // Phase 5 dispatch-shape narrowing (analysis memo). Adding an impl on a
    // *package* type `P` moves only the keys of modules that can dispatch it —
    // the ones that hold a `P`. `shapes` (declares `P`) and `consumer` (holds a
    // `P`) re-check; the two `free*` modules hold no `P` and stay memoized. Under
    // the old build-global dispatch hash every module would have re-checked.
    let dir = write_package(&[
        (
            "shapes.ab",
            "pub unique(A1B2C3D4-0000-4000-8000-000000000042) struct P { x: Number }\n\
             impl P {\n    fn get(self): Number { self.x }\n}\n",
        ),
        (
            "consumer.ab",
            "use pkg::shapes::P;\npub fn run(): Number { P { x: 1 }.get() }\n",
        ),
        ("free1.ab", "pub fn a(): Number { 1 }\n"),
        ("free2.ab", "pub fn b(): Number { 2 }\n"),
    ]);
    let mut session = open(&dir);
    let _ = session.analyze_all();
    let base = session.rechecks();

    // A second, non-colliding inherent impl on `P`, in a brand-new module.
    session.edit_module(
        &module_path("extra"),
        "use pkg::shapes::P;\nimpl P {\n    fn doubled(self): Number { self.x * 2 }\n}\n"
            .to_string(),
    );
    let warm = session.analyze_all();
    assert_eq!(
        session.rechecks() - base,
        3,
        "`extra` (new) + `shapes` (declares P) + `consumer` (holds P) re-check; `free1`/`free2` stay memoized"
    );
    assert_matches_cold(&session, &warm);
}

#[test]
fn trait_signature_change_rechecks_a_module_that_names_the_trait() {
    // "Naming the trait" is covered by the *dependency* channel, not the
    // dispatch key: to name a trait a module must import it, so a change to the
    // trait's definition moves the trait module's interface hash and re-checks
    // every importer. `namer` (`impl Weigh for Item`, importing `Weigh`)
    // re-checks; `item` (declares `Item`, names no trait) and `other` stay
    // memoized — the trait's *impl shapes* did not move.
    let dir = write_package(&[
        (
            "weighmod.ab",
            "pub unique(A1A1A1A1-0000-4000-8000-000000000001) trait Weigh { fn weigh(self): Number; }\n",
        ),
        (
            "item.ab",
            "pub unique(A1A1A1A1-0000-4000-8000-000000000002) struct Item { g: Number }\n",
        ),
        (
            "namer.ab",
            "use pkg::weighmod::Weigh;\nuse pkg::item::Item;\n\
             impl Weigh for Item { fn weigh(self): Number { self.g } }\n",
        ),
        ("other.ab", "pub fn f(): Number { 2 }\n"),
    ]);
    let mut session = open(&dir);
    let _ = session.analyze_all();
    let base = session.rechecks();

    // Change the trait method's return type. The impl in `namer` no longer
    // matches, so `namer` must re-check and surface an error.
    session.edit_module(
        &module_path("weighmod"),
        "pub unique(A1A1A1A1-0000-4000-8000-000000000001) trait Weigh { fn weigh(self): String; }\n"
            .to_string(),
    );
    let warm = session.analyze_all();
    assert_eq!(
        session.rechecks() - base,
        2,
        "`weighmod` (source) + `namer` (imports Weigh) re-check; `item`/`other` stay memoized"
    );
    assert!(
        !diags(&warm["test::namer"]).is_empty(),
        "namer's impl no longer matches the trait: it must report an error"
    );
    assert_matches_cold(&session, &warm);
}

#[test]
fn ability_method_add_rechecks_consumers_only() {
    // Abilities carry no dispatch-key input: the dependency channel covers
    // them, because every performer/handler *names* the ability (a resolve-dep
    // edge to its declaring module). Adding a method to `Counter` moves
    // `effects`' interface hash, so `performer` (performs `Counter::next`) and
    // `handler` (handles it) re-check; `other` names no ability and stays
    // memoized. Under the old build-global ability fold every module re-checked.
    let dir = write_package(&[
        (
            "effects.ab",
            "pub unique(C0DEC0DE-0000-4000-8000-000000000001) ability Counter {\n\
             \x20   fn next(): Number { 0 }\n}\n",
        ),
        (
            "performer.ab",
            "use pkg::effects::Counter;\n\
             pub fn run(): Number with Counter { Counter::next!() }\n",
        ),
        (
            "handler.ab",
            "use pkg::effects::Counter;\n\
             pub fn h(): Number {\n\
             \x20   with { Counter::next() => resume(5) } handle Counter::next!()\n}\n",
        ),
        ("other.ab", "pub fn f(): Number { 2 }\n"),
    ]);
    let mut session = open(&dir);
    let _ = session.analyze_all();
    let base = session.rechecks();

    session.edit_module(
        &module_path("effects"),
        "pub unique(C0DEC0DE-0000-4000-8000-000000000001) ability Counter {\n\
         \x20   fn next(): Number { 0 }\n\
         \x20   fn reset(): Number { 1 }\n}\n"
            .to_string(),
    );
    let warm = session.analyze_all();
    assert_eq!(
        session.rechecks() - base,
        3,
        "`effects` (source) + `performer` + `handler` (name Counter) re-check; `other` stays memoized"
    );
    assert_matches_cold(&session, &warm);
}

#[test]
fn ability_never_flag_flip_rechecks_consumers_only() {
    // Flipping a method to a never method (`: !`) is a shape change carried by
    // the dependency channel: `performer` imports `Abort` (a resolve-dep edge),
    // so it re-checks and its perform now unwinds; `other` names no ability and
    // stays memoized.
    let dir = write_package(&[
        (
            "effects.ab",
            "pub unique(C0DEC0DE-0000-4000-8000-000000000002) ability Abort {\n\
             \x20   fn stop(): Number { 0 }\n}\n",
        ),
        (
            "performer.ab",
            "use pkg::effects::Abort;\n\
             pub fn run(): Number with Abort { Abort::stop!() }\n",
        ),
        ("other.ab", "pub fn f(): Number { 2 }\n"),
    ]);
    let mut session = open(&dir);
    let _ = session.analyze_all();
    let base = session.rechecks();

    session.edit_module(
        &module_path("effects"),
        "pub unique(C0DEC0DE-0000-4000-8000-000000000002) ability Abort {\n\
         \x20   fn stop(): !;\n}\n"
            .to_string(),
    );
    let warm = session.analyze_all();
    assert_eq!(
        session.rechecks() - base,
        2,
        "`effects` (source) + `performer` (names Abort) re-check; `other` stays memoized"
    );
    assert_matches_cold(&session, &warm);
}

#[test]
fn duplicate_impl_add_resurfaces_the_error_in_every_module() {
    // Coherence stays build-global under narrowing: a cold check reports a
    // duplicate `impl Named for W` in *every* module's diagnostics (each seeds a
    // fresh registry with all package impls). So a duplicate add must move every
    // module's key — even a non-holder like `other` — or a memo hit would serve
    // stale, error-free diagnostics. The narrowing promotes both colliding impls
    // to the build-global bytes, so `other` re-checks and re-surfaces the error.
    let dir = write_package(&[
        (
            "defs.ab",
            "pub unique(E0E0E0E0-0000-4000-8000-000000000001) struct W { n: Number }\n\
             pub unique(E0E0E0E0-0000-4000-8000-000000000002) trait Named { fn name(self): Number; }\n",
        ),
        (
            "impl_one.ab",
            "use pkg::defs::W;\nuse pkg::defs::Named;\n\
             impl Named for W { fn name(self): Number { self.n } }\n",
        ),
        ("other.ab", "pub fn f(): Number { 2 }\n"),
    ]);
    let mut session = open(&dir);
    let cold = session.analyze_all();
    assert!(
        diags(&cold["test::other"]).is_empty(),
        "before the duplicate, `other` is clean"
    );

    // A duplicate impl in a brand-new, otherwise-unrelated module.
    session.edit_module(
        &module_path("impl_two"),
        "use pkg::defs::W;\nuse pkg::defs::Named;\n\
         impl Named for W { fn name(self): Number { self.n + 1 } }\n"
            .to_string(),
    );
    let warm = session.analyze_all();
    assert!(
        diags(&warm["test::other"])
            .iter()
            .any(|(_, _, m)| m.contains("duplicate implementation")),
        "the coherence error must resurface in the non-holder `other`, got {:?}",
        diags(&warm["test::other"])
    );
    assert_matches_cold(&session, &warm);
}

#[test]
fn import_cycle_appears_and_clears_across_edits() {
    // Start acyclic, introduce a cycle via a body edit (no interface
    // change), then remove it. Cycles are recomputed per revision, so both
    // transitions match a cold analysis.
    let dir = write_package(&[
        ("a.ab", "pub fn ay(): Number { 1 }\n"),
        ("b.ab", "use pkg::a::ay;\npub fn bee(): Number { ay() }\n"),
    ]);
    let mut session = open(&dir);
    let clean = session.analyze_all();
    assert!(clean["test::a"].import_cycle.is_none());
    assert_matches_cold(&session, &clean);

    // `a` now imports `b`: a -> b -> a. `ay`'s signature is unchanged.
    session.edit_module(
        &module_path("a"),
        "use pkg::b::bee;\npub fn ay(): Number { bee() }\n".to_string(),
    );
    let cyclic = session.analyze_all();
    assert!(
        cyclic["test::a"].import_cycle.is_some(),
        "cycle must surface"
    );
    assert!(cyclic["test::b"].import_cycle.is_some());
    assert_matches_cold(&session, &cyclic);

    // Break the cycle again.
    session.edit_module(&module_path("a"), "pub fn ay(): Number { 1 }\n".to_string());
    let healed = session.analyze_all();
    assert!(healed["test::a"].import_cycle.is_none(), "cycle must clear");
    assert_matches_cold(&session, &healed);
}

#[test]
fn broken_module_does_not_poison_siblings_and_recovers() {
    let dir = write_package(&[
        (
            "main.ab",
            "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
        ),
        ("utils.ab", "pub fn helper(): Number { 1 }\n"),
    ]);
    let mut session = open(&dir);
    let _ = session.analyze_all();

    // Break `main` mid-edit (unparseable). `utils` must stay clean.
    session.edit_module(
        &module_path("main"),
        "pub fn run(: Number { helper(\n".to_string(),
    );
    let broken = session.analyze_all();
    assert!(
        !broken["test"].diagnostics().is_empty(),
        "broken main reports"
    );
    assert!(
        broken["test::utils"].diagnostics().is_empty(),
        "sibling not poisoned: {:?}",
        diags(&broken["test::utils"])
    );
    assert_matches_cold(&session, &broken);

    // Fix it: the whole package is clean again, matching cold.
    session.edit_module(
        &module_path("main"),
        "use pkg::utils::helper;\npub fn run(): Number { helper() }\n".to_string(),
    );
    let healed = session.analyze_all();
    for (key, r) in &healed {
        assert!(r.diagnostics().is_empty(), "{key}: {:?}", diags(r));
    }
    assert_matches_cold(&session, &healed);
}

#[test]
fn span_shifting_edit_is_never_served_stale() {
    // Regression: the memo key hashes the raw source, not the span-free
    // structural AST hash. A leading-newline edit shifts a type error's
    // span without changing the module's structure; a span-free key would
    // replay the old span. The warm result must track the shift exactly.
    let dir = write_package(&[("main.ab", "pub fn run(): String { 42 }\n")]);
    let mut session = open(&dir);
    let before = session.analyze_all();
    let before_span = before["test"].diagnostics()[0].span.start;

    session.edit_module(
        &module_path("main"),
        "\n\npub fn run(): String { 42 }\n".to_string(),
    );
    let after = session.analyze_all();
    let after_span = after["test"].diagnostics()[0].span.start;

    assert_eq!(
        after_span,
        before_span + 2,
        "the type error span must shift"
    );
    assert_matches_cold(&session, &after);
}

#[test]
fn analyze_module_and_analyze_all_agree() {
    let dir = write_package(&[
        (
            "main.ab",
            "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
        ),
        ("utils.ab", "pub fn helper(): Number { 1 }\n"),
    ]);
    let mut session = open(&dir);
    let all = session.analyze_all();
    let one = session.analyze_module(&module_path("main")).expect("main");
    assert_eq!(diags(&one), diags(&all["test"]));
}

#[test]
fn workspace_symbols_work_cold_without_snapshot() {
    // No build has run: the store is absent. `load_snapshot` is a silent
    // no-op and symbol search serves the live analysis state.
    let dir = write_package(&[("utils.ab", "pub fn helper(): Number { 1 }\n")]);
    let mut session = open(&dir);
    session.load_snapshot(); // no store on disk — leaves the index empty
    assert!(session.snapshot.is_none(), "cold workspace has no snapshot");
    let names: Vec<_> = session
        .workspace_symbols("helper")
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, vec!["helper"]);
}

#[test]
fn live_edit_supersedes_a_stale_on_disk_snapshot() {
    use ambient_engine::disk_store::{
        BuildManifest, DiskStore, MANIFEST_VERSION, ManifestItem, ManifestModule,
    };
    use ambient_engine::module_interface::ItemKindTag;

    let dir = write_package(&[("utils.ab", "pub fn helper(): Number { 1 }\n")]);
    let mut session = open(&dir);
    let _ = session.analyze_all();

    // A snapshot reflecting the last build, where `utils` still exposed
    // `helper` — written to the real store and loaded read-only.
    let utils_key = session
        .interfaces
        .keys()
        .find(|k| k.ends_with("utils"))
        .expect("utils interface")
        .clone();
    let manifest = BuildManifest {
        version: MANIFEST_VERSION,
        package_name: "test".to_string(),
        dispatch_surface_hash: [0u8; 32],
        natives_contract_hash: [0u8; 32],
        core_cache_key: [0u8; 32],
        modules: vec![ManifestModule {
            module: utils_key,
            resolved_ast_hash: [0u8; 32],
            interface_hash: [0u8; 32],
            deps: vec![],
            objects: vec![],
            names: vec![],
            signatures: vec![],
            cache_key: [0u8; 32],
            consumed_links: vec![],
            migrations: vec![],
            lambda_parents: vec![],
            entry_point: None,
            source_path: "utils.ab".to_string(),
            items: vec![ManifestItem {
                ident: vec!["helper".to_string()],
                kind: ItemKindTag::Function,
                hash: None,
                uuid: String::new(),
                span: (0, 1),
                summary: String::new(),
            }],
            prelink: None,
        }],
    };
    let store = DiskStore::open_package(dir.path()).expect("open store");
    store.write_snapshot(&manifest).expect("write snapshot");
    session.load_snapshot();
    assert!(session.snapshot.is_some(), "snapshot must load from disk");

    // Rename the live symbol; the buffer is now ahead of the snapshot.
    session.edit_module(
        &module_path("utils"),
        "pub fn renamed(): Number { 1 }\n".to_string(),
    );

    let names: Vec<_> = session
        .workspace_symbols("")
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(
        names.contains(&"renamed".to_string()),
        "live symbol: {names:?}"
    );
    assert!(
        !names.contains(&"helper".to_string()),
        "stale snapshot symbol served for a live module: {names:?}"
    );
}

// ── occurrence index (find-references / rename) ─────────────────────────────

/// Every occurrence of the symbol whose name is `name` and that is *defined*
/// in module `def_module`, gathered across the whole package by identity —
/// exactly what find-references renders. Returns `(module_key, span, is_def)`.
fn references_to(
    session: &AnalysisSession,
    def_module: &str,
    name: &str,
) -> Vec<(String, u32, u32, bool)> {
    // The identity is the target on the definition occurrence in `def_module`.
    let def_occ = session
        .occurrences_for(&module_path(def_module))
        .expect("def module occurrences")
        .iter()
        .find(|o| o.is_definition && o.target.name().as_ref() == name)
        .expect("definition occurrence");
    let target = def_occ.target.clone();

    let mut out = Vec::new();
    for m in session.package().modules.values() {
        let key = m.path.to_string();
        for occ in session.occurrences_for(&m.path).unwrap_or(&[]) {
            if occ.target == target {
                out.push((key.clone(), occ.span.start, occ.span.end, occ.is_definition));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn body_only_edit_rebuilds_exactly_one_modules_occurrences() {
    let dir = write_package(&[
        (
            "main.ab",
            "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
        ),
        ("utils.ab", "pub fn helper(): Number { 1 }\n"),
        ("other.ab", "pub fn unrelated(): Number { 2 }\n"),
    ]);
    let mut session = open(&dir);
    let base = session.occurrence_rebuilds();

    // A body-only edit to `main` (interface unchanged): only `main`'s
    // occurrences are re-collected; `utils`/`other` entries are left intact.
    session.edit_module(
        &module_path("main"),
        "use pkg::utils::helper;\npub fn run(): Number { helper() + 0 }\n".to_string(),
    );
    assert_eq!(
        session.occurrence_rebuilds() - base,
        1,
        "exactly one module's occurrences re-collected on a body-only edit"
    );
}

#[test]
fn cross_module_references_survive_a_span_shifting_edit() {
    // The stranded-reference guard: a span-shifting body edit in the *defining*
    // module must not orphan another module's references to it. Under the old
    // span-keyed identity, `main`'s (un-rebuilt) reference would point at the
    // stale definition span; Fqn keying keeps them collapsed.
    let dir = write_package(&[
        (
            "main.ab",
            "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
        ),
        ("utils.ab", "pub fn helper(): Number { 1 }\n"),
    ]);
    let mut session = open(&dir);

    let before = references_to(&session, "utils", "helper");
    // def in utils + import in main + call in main = 3 sites.
    assert_eq!(before.len(), 3, "baseline references: {before:?}");
    assert!(
        before
            .iter()
            .any(|(m, _, _, is_def)| m == "test" && !is_def)
    );

    // Shift `helper`'s definition span with two leading newlines. The signature
    // is unchanged, so this is a body-only edit and `main` is NOT re-collected.
    let rebuilds_before = session.occurrence_rebuilds();
    session.edit_module(
        &module_path("utils"),
        "\n\npub fn helper(): Number { 1 }\n".to_string(),
    );
    assert_eq!(
        session.occurrence_rebuilds() - rebuilds_before,
        1,
        "only `utils` re-collected"
    );

    let after = references_to(&session, "utils", "helper");
    // Still three sites, and `main`'s call is still among them — not stranded.
    assert_eq!(after.len(), 3, "references after the edit: {after:?}");
    assert!(
        after.iter().any(|(m, _, _, is_def)| m == "test" && !is_def),
        "main's cross-module reference survived the span shift: {after:?}"
    );
    // The definition span in utils moved (proving the edit landed), yet the
    // reference set still resolves onto it.
    let def_before = before.iter().find(|(m, ..)| m == "test::utils").unwrap();
    let def_after = after.iter().find(|(m, ..)| m == "test::utils").unwrap();
    assert_ne!(
        (def_before.1, def_before.2),
        (def_after.1, def_after.2),
        "the def span must have shifted"
    );
}

#[test]
fn scoped_occurrence_rebuild_matches_a_full_rebuild() {
    // The occurrence oracle, driven directly: after a scoped rebuild the
    // incrementally-maintained index must equal a full cold re-collection of
    // every module. `assert_occurrences_scoped_matches_full` panics on
    // divergence; call it explicitly (independent of the verify env flag).
    let dir = write_package(&[
        (
            "main.ab",
            "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
        ),
        ("utils.ab", "pub fn helper(): Number { 1 }\n"),
        ("other.ab", "pub fn unrelated(): Number { 2 }\n"),
    ]);
    let mut session = open(&dir);

    session.edit_module(
        &module_path("utils"),
        "\n\npub fn helper(): Number { 1 }\n".to_string(),
    );
    session.assert_occurrences_scoped_matches_full();

    session.edit_module(
        &module_path("main"),
        "use pkg::utils::helper;\npub fn run(): Number { helper() + 0 }\n".to_string(),
    );
    session.assert_occurrences_scoped_matches_full();
}

#[test]
fn occurrence_verify_oracle_passes_on_body_edits() {
    // With verify on, `edit_module`'s body-only path runs the occurrence oracle
    // internally. A clean pass proves the scoped rebuild is equivalent to a full
    // one; a stranded reference would panic inside `edit_module`.
    let dir = write_package(&[
        (
            "main.ab",
            "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
        ),
        ("utils.ab", "pub fn helper(): Number { 1 }\n"),
    ]);
    let mut session = open(&dir);
    session.verify = true;

    session.edit_module(
        &module_path("utils"),
        "\n\npub fn helper(): Number { 1 }\n".to_string(),
    );
    session.edit_module(
        &module_path("main"),
        "use pkg::utils::helper;\npub fn run(): Number { helper() + 0 }\n".to_string(),
    );
}

/// A package with an enum variant declared in one module and referenced (as a
/// constructor and a pattern) from two others.
fn variant_package() -> TempDir {
    write_package(&[
        (
            "shapes.ab",
            "pub unique(A1B2C3D4-0000-0000-0000-0000000000C1) enum Shape \
             { Circle(Number), Square }\n",
        ),
        (
            "main.ab",
            "use pkg::shapes::Shape;\npub fn mk(): Shape { Shape::Circle(2.0) }\n",
        ),
        (
            "other.ab",
            "use pkg::shapes::Shape;\n\
             pub fn area(s: Shape): Number { match s { Shape::Circle(n) => n, Square => 0 } }\n",
        ),
    ])
}

#[test]
fn cross_module_variant_references_are_found() {
    // The correctness bar for variant indexing: a variant's declaration, a
    // constructor in another module, and a pattern site in a third all collapse
    // onto the one `[Enum, Variant]` identity.
    let session = open(&variant_package());
    let refs = references_to(&session, "shapes", "Circle");
    assert_eq!(refs.len(), 3, "variant sites across the package: {refs:?}");
    assert!(
        refs.iter()
            .any(|(m, _, _, is_def)| m == "test::shapes" && *is_def),
        "declaration in shapes: {refs:?}"
    );
    assert!(
        refs.iter().any(|(m, _, _, is_def)| m == "test" && !is_def),
        "constructor in main: {refs:?}"
    );
    assert!(
        refs.iter()
            .any(|(m, _, _, is_def)| m == "test::other" && !is_def),
        "pattern in other: {refs:?}"
    );
    // The variant identity is distinct from the enum's — renaming one never
    // touches the other.
    let enum_refs = references_to(&session, "shapes", "Shape");
    assert!(
        enum_refs.iter().all(|site| !refs.contains(site)),
        "enum and variant occurrence sets are disjoint"
    );
}

#[test]
fn variant_occurrence_oracle_survives_a_body_edit() {
    // The scoped-rebuild oracle must stay green with variant occurrences
    // present: a body-only edit in one referencing module re-collects only it,
    // yet the cross-module variant references stay collapsed.
    let mut session = open(&variant_package());
    session.verify = true;

    let before = references_to(&session, "shapes", "Circle");
    assert_eq!(before.len(), 3, "{before:?}");

    // A span-shifting body-only edit to `main` (interface unchanged).
    session.edit_module(
        &module_path("main"),
        "use pkg::shapes::Shape;\n\npub fn mk(): Shape { Shape::Circle(2.0) }\n".to_string(),
    );
    session.assert_occurrences_scoped_matches_full();

    let after = references_to(&session, "shapes", "Circle");
    assert_eq!(after.len(), 3, "references survived the edit: {after:?}");
    assert!(
        after.iter().any(|(m, _, _, is_def)| m == "test" && !is_def),
        "main's constructor is not stranded: {after:?}"
    );
}

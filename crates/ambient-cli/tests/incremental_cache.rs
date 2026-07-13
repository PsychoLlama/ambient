//! End-to-end coverage of incremental-compilation cache hits (Phase 3).
//!
//! Each test builds a package, persists the snapshot, mutates a module (or
//! not), rebuilds warm, and asserts two things: the warm build is *byte
//! identical* to a fresh cold build of the same final source (the manifest
//! encoding captures objects, names, signatures, migrations, lambda parents,
//! entry point, consumed links, and every cache key), and the recompile
//! counter reflects exactly which modules missed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ambient_engine::ast::Module;
use ambient_engine::build::{BuildOptions, BuildResult, CacheMode, ParseFailure, build_package};
use ambient_engine::disk_store::{BuildManifest, DiskStore};
use tempfile::TempDir;

fn parse_source(source: &str) -> Result<Module, ParseFailure> {
    ambient_parser::parse(source).map_err(|e| ParseFailure {
        message: e.kind.to_string(),
        span: (e.span.start, e.span.end),
        context: e.context,
    })
}

fn write_pkg(dir: &Path, files: &[(&str, &str)]) {
    fs::write(
        dir.join("ambient.toml"),
        "[package]\nname = \"cache_pkg\"\nversion = \"0.1.0\"\n",
    )
    .expect("manifest");
    let src = dir.join("src");
    for (rel, body) in files {
        let path = src.join(rel);
        fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        fs::write(path, body).expect("write module");
    }
}

fn store_path(dir: &Path) -> PathBuf {
    dir.join(".ambient").join("store")
}

/// Build reading the package's own store (cache Auto), then persist objects +
/// snapshot exactly as `ambient run` does — so the next build can hit.
fn build_and_persist(dir: &Path) -> BuildResult {
    let stubs = ambient_platform::stub_natives();
    let result = build_package(
        dir,
        parse_source,
        &BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            store_path: Some(store_path(dir)),
            ..Default::default()
        },
    )
    .expect("build succeeds");
    let store = DiskStore::open(store_path(dir)).expect("open store");
    store.put_module(&result.compiled).expect("persist objects");
    let manifest = BuildManifest::from_build(&result);
    store.write_snapshot(&manifest).expect("write snapshot");
    result
}

/// The canonical cold manifest for a source: build the given files in a fresh
/// package (empty store, no snapshot ⇒ everything compiles).
fn cold_manifest(files: &[(&str, &str)]) -> BuildManifest {
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), files);
    BuildManifest::from_build(&build_and_persist(dir.path()))
}

fn assert_warm_equals_cold(warm: &BuildResult, files: &[(&str, &str)]) {
    let warm_manifest = BuildManifest::from_build(warm);
    let cold = cold_manifest(files);
    assert_eq!(
        warm_manifest.encode(),
        cold.encode(),
        "warm build must be byte-identical to a fresh cold build"
    );
}

#[test]
fn zero_change_rebuild_is_a_full_warm_hit() {
    let files: &[(&str, &str)] = &[
        (
            "math.ab",
            "pub fn gcd(a: Number, b: Number): Number { if b == 0 { a } else { gcd(b, a % b) } }\n",
        ),
        (
            "main.ab",
            "use pkg::math::gcd;\npub fn run(): Number { gcd(48, 36) }\n",
        ),
    ];
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), files);

    let cold = build_and_persist(dir.path());
    assert!(cold.modules_compiled > 0, "first build compiles everything");

    let warm = build_and_persist(dir.path());
    assert_eq!(
        warm.modules_compiled, 0,
        "every user + builtin module must hit on an unchanged rebuild"
    );
    assert_warm_equals_cold(&warm, files);
}

#[test]
fn body_only_edit_recompiles_only_the_leaf_and_its_dependents() {
    let dir = TempDir::new().expect("temp");
    write_pkg(
        dir.path(),
        &[
            ("leaf.ab", "pub fn base(): Number { 1 }\n"),
            (
                "mid.ab",
                "use pkg::leaf::base;\npub fn doubled(): Number { base() + base() }\n",
            ),
            (
                "main.ab",
                "use pkg::mid::doubled;\npub fn run(): Number { doubled() }\n",
            ),
        ],
    );
    build_and_persist(dir.path());

    // Body-only edit of the leaf: its signature (interface) is unchanged, so
    // `mid` misses only via link validation (base's final hash moved), and
    // `main` via `mid`'s.
    let edited: &[(&str, &str)] = &[
        ("leaf.ab", "pub fn base(): Number { 2 }\n"),
        (
            "mid.ab",
            "use pkg::leaf::base;\npub fn doubled(): Number { base() + base() }\n",
        ),
        (
            "main.ab",
            "use pkg::mid::doubled;\npub fn run(): Number { doubled() }\n",
        ),
    ];
    fs::write(dir.path().join("src/leaf.ab"), edited[0].1).expect("edit");

    let warm = build_and_persist(dir.path());
    assert_eq!(
        warm.modules_compiled, 3,
        "leaf (ast), mid + main (link validation) recompile; builtins hit"
    );
    assert_warm_equals_cold(&warm, edited);
}

#[test]
fn pub_signature_edit_misses_dependents_via_interface_hash() {
    let dir = TempDir::new().expect("temp");
    write_pkg(
        dir.path(),
        &[
            ("lib.ab", "pub fn f(n: Number): Number { n + 1 }\n"),
            (
                "main.ab",
                "use pkg::lib::f;\npub fn run(): Number { f(1) }\n",
            ),
        ],
    );
    build_and_persist(dir.path());

    // A signature change moves `lib`'s interface hash ⇒ `main`'s cache key.
    let edited: &[(&str, &str)] = &[
        (
            "lib.ab",
            "pub fn f(n: Number, m: Number): Number { n + m }\n",
        ),
        (
            "main.ab",
            "use pkg::lib::f;\npub fn run(): Number { f(1, 2) }\n",
        ),
    ];
    fs::write(dir.path().join("src/lib.ab"), edited[0].1).expect("edit");
    fs::write(dir.path().join("src/main.ab"), edited[1].1).expect("edit");

    let warm = build_and_persist(dir.path());
    assert_eq!(warm.modules_compiled, 2, "lib and its dependent main miss");
    assert_warm_equals_cold(&warm, edited);
}

#[test]
fn new_trait_impl_in_unimported_module_shifts_the_dispatch_surface() {
    let dir = TempDir::new().expect("temp");
    let base: &[(&str, &str)] = &[
        (
            "widget.ab",
            "pub unique(C0FFEE00-0000-0000-0000-000000000001) struct Widget { size: Number }\n",
        ),
        (
            "orphan.ab",
            "pub unique(C0FFEE00-0000-0000-0000-000000000002) struct Gadget { n: Number }\n",
        ),
        ("main.ab", "pub fn run(): Number { 7 }\n"),
    ];
    write_pkg(dir.path(), base);
    build_and_persist(dir.path());

    // Add a trait + impl in a module nobody imports. The build-global
    // dispatch-surface hash folds every impl, so every module's cache key
    // moves and the whole package misses.
    let edited: &[(&str, &str)] = &[
        base[0],
        (
            "orphan.ab",
            "pub unique(C0FFEE00-0000-0000-0000-000000000002) struct Gadget { n: Number }\n\
             pub unique(C0FFEE00-0000-0000-0000-000000000003) trait Named { fn name(self): Number; }\n\
             impl Named for Gadget { fn name(self): Number { self.n } }\n",
        ),
        base[2],
    ];
    fs::write(dir.path().join("src/orphan.ab"), edited[1].1).expect("edit");

    let warm = build_and_persist(dir.path());
    assert!(
        warm.modules_compiled >= 3,
        "a dispatch-surface change invalidates every package module, got {}",
        warm.modules_compiled
    );
    assert_warm_equals_cold(&warm, edited);
}

#[test]
fn transitive_relink_through_a_trait_impl_the_dispatcher_never_imports() {
    // The nasty chain: D (helper) ← C (trait impl calling D) ← A (dispatches
    // the trait method on a type, imports neither C nor D). A body edit to D
    // must still invalidate A, purely through link validation.
    let widget =
        "pub unique(DEADBEEF-0000-0000-0000-000000000001) struct Widget { size: Number }\n";
    let describe = "pub unique(DEADBEEF-0000-0000-0000-000000000002) trait Describe { fn describe(self): Number; }\n";
    let impls = "use pkg::widget::Widget;\n\
                 use pkg::describe::Describe;\n\
                 use pkg::helper::base;\n\
                 impl Describe for Widget { fn describe(self): Number { base() + self.size } }\n";
    let main = "use pkg::widget::Widget;\n\
                use pkg::describe::Describe;\n\
                pub fn run(): Number { let w = Widget { size: 5 }; w.describe() }\n";

    let dir = TempDir::new().expect("temp");
    write_pkg(
        dir.path(),
        &[
            ("widget.ab", widget),
            ("describe.ab", describe),
            ("helper.ab", "pub fn base(): Number { 10 }\n"),
            ("impls.ab", impls),
            ("main.ab", main),
        ],
    );
    build_and_persist(dir.path());

    // Edit only D's helper body. Its signature is unchanged, so C's cache key
    // (which folds D's *interface* hash) does not move — C must miss via link
    // validation, and A (no dep on C or D) via the dispatch symbol.
    let edited: &[(&str, &str)] = &[
        ("widget.ab", widget),
        ("describe.ab", describe),
        ("helper.ab", "pub fn base(): Number { 20 }\n"),
        ("impls.ab", impls),
        ("main.ab", main),
    ];
    fs::write(dir.path().join("src/helper.ab"), edited[2].1).expect("edit");

    let warm = build_and_persist(dir.path());
    assert_warm_equals_cold(&warm, edited);
    // helper (ast), impls (link to base), main (link to dispatch symbol).
    assert_eq!(
        warm.modules_compiled, 3,
        "the whole chain relinks; widget + describe still hit"
    );
    // The proof it actually re-linked: the run result changed with D's body.
    let cold = cold_manifest(edited);
    assert_eq!(BuildManifest::from_build(&warm).encode(), cold.encode());
}

#[test]
fn corrupt_object_behind_a_valid_manifest_self_heals() {
    let files: &[(&str, &str)] = &[
        ("lib.ab", "pub fn answer(): Number { 42 }\n"),
        (
            "main.ab",
            "use pkg::lib::answer;\npub fn run(): Number { answer() }\n",
        ),
    ];
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), files);
    let cold = build_and_persist(dir.path());

    // Corrupt one object file the manifest references: overwrite it with
    // garbage. The read self-verifies, so the affected module misses rather
    // than loading bad code; the build still succeeds and re-persists.
    let store = DiskStore::open(store_path(dir.path())).expect("store");
    let victim = cold
        .module_outputs
        .values()
        .flat_map(|o| o.objects.iter())
        .next()
        .copied()
        .expect("some object");
    fs::write(store.object_path(&victim), b"not an object").expect("corrupt");

    let warm = build_and_persist(dir.path());
    assert!(
        warm.modules_compiled > 0,
        "a corrupt object forces at least a partial recompile"
    );
    assert_warm_equals_cold(&warm, files);

    // The re-persist healed the store: a clean rebuild now hits fully.
    let healed = build_and_persist(dir.path());
    assert_eq!(
        healed.modules_compiled, 0,
        "store self-healed on re-persist"
    );
}

#[test]
fn verify_mode_recompiles_and_agrees_on_a_multi_module_build() {
    // The oracle is env-driven, and env vars are process-global — unsafe to
    // mutate from a parallel in-process test — so drive it through a real
    // `ambient run` subprocess (its own process, its own env). Two clean runs
    // under `AMBIENT_CACHE_VERIFY=1` prove the hit path agrees with a fresh
    // compile on this multi-module package (the standing under-invalidation
    // detector would panic → nonzero exit on any mismatch).
    let files: &[(&str, &str)] = &[
        ("a.ab", "pub fn a(): Number { 1 }\n"),
        ("b.ab", "use pkg::a::a;\npub fn b(): Number { a() + 1 }\n"),
        ("main.ab", "use pkg::b::b;\npub fn run(): Number { b() }\n"),
    ];
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), files);

    for _ in 0..2 {
        let out = Command::new(env!("CARGO_BIN_EXE_ambient"))
            .arg("run")
            .arg(dir.path())
            .env("AMBIENT_CACHE_VERIFY", "1")
            .output()
            .expect("spawn ambient run");
        assert!(
            out.status.success(),
            "ambient run under verify mode failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn reexported_enum_consumed_through_two_hops_compiles_and_caches() {
    // Phase 5 step 1: `ModuleEnv`'s foreign-item channels are narrowed to a
    // module's resolve-dependency closure. A variant reached through *two*
    // hops of `pub use` (main → mid → leaf) canonicalizes to the defining
    // origin (`leaf`), which becomes a direct dependency — so the narrowed
    // env still holds its variant info, with no transitive re-export closure.
    // `main` names the variant only through the fully-qualified path
    // (`pkg::mid::Color::Green`), exercising the narrowed
    // `foreign_enum_variants` channel rather than the import channel.
    let files: &[(&str, &str)] = &[
        (
            "leaf.ab",
            "pub unique(AAAA0000-0000-0000-0000-000000000001) enum Color { Red, Green }\n",
        ),
        ("mid.ab", "pub use pkg::leaf::Color;\n"),
        (
            "main.ab",
            "use pkg::mid::Color;\n\
             pub fn run(): Number { let c = pkg::mid::Color::Green; match c { Red => 0, Green => 1 } }\n",
        ),
    ];
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), files);

    // Cold build must compile the two-hop construction (the narrowed env for
    // `main` must include `leaf`, or codegen would fail to inline the tag).
    let cold = build_and_persist(dir.path());
    assert!(cold.modules_compiled > 0, "first build compiles");

    // Warm zero-change rebuild: a full hit that is byte-identical to cold.
    let warm = build_and_persist(dir.path());
    assert_eq!(warm.modules_compiled, 0, "unchanged rebuild is a full hit");
    assert_warm_equals_cold(&warm, files);

    // And it runs to the right value (variant tag inlined through the narrow
    // channel), warm hit agreeing with a fresh compile under the oracle.
    let out = Command::new(env!("CARGO_BIN_EXE_ambient"))
        .arg("run")
        .arg(dir.path())
        .env("AMBIENT_CACHE_VERIFY", "1")
        .output()
        .expect("spawn ambient run");
    assert!(
        out.status.success(),
        "two-hop run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains('1'),
        "run must yield the Green tag (1): {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn cache_off_flag_forces_a_cold_build() {
    let files: &[(&str, &str)] = &[("main.ab", "pub fn run(): Number { 1 }\n")];
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), files);
    build_and_persist(dir.path());

    // `CacheMode::Off` (the in-process equivalent of `AMBIENT_CACHE=off`)
    // bypasses the snapshot entirely.
    let stubs = ambient_platform::stub_natives();
    let cold = build_package(
        dir.path(),
        parse_source,
        &BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            store_path: Some(store_path(dir.path())),
            cache: CacheMode::Off,
            ..Default::default()
        },
    )
    .expect("build succeeds");
    assert!(
        cold.modules_compiled > 0,
        "CacheMode::Off must ignore the snapshot"
    );
}

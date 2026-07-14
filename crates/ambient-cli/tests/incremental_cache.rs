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

use ambient_engine::build::{BuildOptions, BuildResult, CacheMode, build_package};
use ambient_engine::disk_store::{BuildManifest, DiskStore};
use tempfile::TempDir;

mod common;
use common::{build_and_persist, parse_source, verify_mode, write_pkg_named};

/// The package name every incremental-cache test builds under. A warm build and
/// its cold twin must share it, or their module ids (and manifests) diverge.
const PKG: &str = "cache_pkg";

fn write_pkg(dir: &Path, files: &[(&str, &str)]) {
    write_pkg_named(dir, PKG, files);
}

/// The canonical cold manifest for a source (built under [`PKG`]).
fn cold_manifest(files: &[(&str, &str)]) -> BuildManifest {
    common::cold_manifest(PKG, files)
}

fn assert_warm_equals_cold(warm: &BuildResult, files: &[(&str, &str)]) {
    common::assert_warm_equals_cold(PKG, warm, files);
}

fn store_path(dir: &Path) -> PathBuf {
    dir.join(".ambient").join("store")
}

/// Build-and-persist while capturing each module's `from_cache` flag from the
/// progress callback, so a test can assert *which* modules were served warm
/// (not merely how many). Keyed by the module's dotted path (its basename for a
/// top-level module).
fn build_capturing(dir: &Path) -> (BuildResult, std::collections::HashMap<String, bool>) {
    use std::cell::RefCell;
    let seen: RefCell<std::collections::HashMap<String, bool>> = RefCell::new(Default::default());
    let stubs = ambient_platform::stub_natives();
    let result = {
        let cb = |name: &str, _c: usize, _t: usize, from_cache: bool| {
            seen.borrow_mut().insert(name.to_string(), from_cache);
        };
        let built = ambient_engine::build::build_and_persist(
            dir,
            parse_source,
            BuildOptions {
                platform_modules: ambient_platform::platform_modules(),
                natives: Some(&stubs),
                progress: Some(&cb),
                ..Default::default()
            },
        )
        .expect("build succeeds");
        built.persisted.expect("persist build");
        built.result
    };
    (result, seen.into_inner())
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
    assert!(
        cold.modules_checked > 0,
        "a cold build type-checks every missing module"
    );

    let warm = build_and_persist(dir.path());
    if !verify_mode() {
        assert_eq!(
            warm.modules_compiled, 0,
            "every user + builtin module must hit on an unchanged rebuild"
        );
        assert_eq!(
            warm.modules_relinked, 0,
            "an unchanged rebuild needs no relink — every module is a full hit"
        );
        // The check pre-pass skips every key-match module, and a full hit never
        // reaches the walk's lazy fallback, so a warm build re-checks nothing.
        assert_eq!(
            warm.modules_checked, 0,
            "a full warm hit must perform zero type-checks"
        );
    }
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
    // Only the edited leaf re-checks+compiles (its AST hash moved). `mid` and
    // `main` fail link validation — base's final hash moved — but their cache
    // keys still match, so they take the relink fast path: remap the moved
    // foreign hash and re-finalize, with no re-check and no codegen. The check
    // counter for both dependents is zero.
    if !verify_mode() {
        assert_eq!(
            warm.modules_compiled, 1,
            "only the leaf re-checks+compiles; dependents relink"
        );
        assert_eq!(
            warm.modules_relinked, 2,
            "mid + main relink (link-only miss, key match)"
        );
        assert_eq!(
            warm.modules_checked, 1,
            "only the leaf re-checks; the relinked dependents (key match) do not"
        );
    }
    // The relinked build is byte-identical to a fresh cold build of the final
    // source — objects, names, signatures, consumed links, and each module's
    // prelink-blob hash all match.
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
    if !verify_mode() {
        assert_eq!(warm.modules_compiled, 2, "lib and its dependent main miss");
    }
    assert_warm_equals_cold(&warm, edited);
}

#[test]
fn new_trait_impl_on_a_package_type_spares_unrelated_modules() {
    // Phase 5 dispatch-shape narrowing: adding a trait + impl on a *package*
    // type `Gadget` in a module nobody imports invalidates only modules that
    // can dispatch it — i.e. hold a `Gadget`. `widget` (an unrelated struct) and
    // `main` (`run` → 7) hold no `Gadget` and depend on no module that declares
    // one, so their narrowed dispatch key is unchanged and they stay warm hits.
    // Only `orphan` (its own source moved) recompiles. Under the old build-global
    // dispatch-surface hash the whole package would have re-checked.
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
    if !verify_mode() {
        assert_eq!(
            warm.modules_compiled, 1,
            "only orphan (whose source moved) recompiles; widget + main stay cached, got {}",
            warm.modules_compiled
        );
    }
    assert_warm_equals_cold(&warm, edited);
}

#[test]
fn operator_dispatcher_recompiles_when_a_new_impl_lands_on_the_operated_type() {
    // The operator edge case: `a + b` desugars to the reserved `Add` trait
    // without ever naming `Add`, so a trait-name-based narrowing would miss it.
    // The narrowing keys on the *receiver type* instead: `calc` holds a `Widget`
    // (it does `w1 + w2`), so its dispatch key folds every impl on `Widget` —
    // including the orphan `Add for Widget` in `ops`, which `calc` never imports.
    // Adding a *second* impl on `Widget` (a `Sub`) therefore re-checks `main`
    // (which does `w1 + w2`), while the two truly-unrelated modules stay warm.
    let widget = "pub unique(FEED0000-0000-0000-0000-000000000001) struct Widget { x: Number }\n";
    let ops = "use pkg::widget::Widget;\n\
               impl Add for Widget { fn add(self, o: Widget): Widget { Widget { x: self.x + o.x } } }\n";
    let main = "use pkg::widget::Widget;\n\
                pub fn run(): Number { (Widget { x: 1 } + Widget { x: 2 }).x }\n";
    let base: &[(&str, &str)] = &[
        ("widget.ab", widget),
        ("ops.ab", ops),
        ("main.ab", main),
        ("free1.ab", "pub fn a(): Number { 1 }\n"),
        ("free2.ab", "pub fn b(): Number { 2 }\n"),
    ];
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), base);
    build_and_persist(dir.path());

    let ops_edited = "use pkg::widget::Widget;\n\
                      impl Add for Widget { fn add(self, o: Widget): Widget { Widget { x: self.x + o.x } } }\n\
                      impl Sub for Widget { fn sub(self, o: Widget): Widget { Widget { x: self.x - o.x } } }\n";
    fs::write(dir.path().join("src/ops.ab"), ops_edited).expect("edit");
    let edited: &[(&str, &str)] = &[
        ("widget.ab", widget),
        ("ops.ab", ops_edited),
        ("main.ab", main),
        ("free1.ab", "pub fn a(): Number { 1 }\n"),
        ("free2.ab", "pub fn b(): Number { 2 }\n"),
    ];

    let (warm, seen) = build_capturing(dir.path());
    if !verify_mode() {
        assert_eq!(
            seen.get("main"),
            Some(&false),
            "main dispatches `+` on Widget, so a new impl on Widget re-checks it"
        );
        assert_eq!(
            seen.get("free1"),
            Some(&true),
            "free1 holds no Widget: warm"
        );
        assert_eq!(
            seen.get("free2"),
            Some(&true),
            "free2 holds no Widget: warm"
        );
    }
    assert_warm_equals_cold(&warm, edited);
}

#[test]
fn a_new_impl_recompiles_the_concrete_call_site_but_not_the_generic_function() {
    // Dictionary passing: a bounded generic `use_describe<T: Describe>` receives
    // its `Describe` dictionary from callers, so it links no concrete impl symbol
    // — the *call site* that instantiates the bound at a concrete `Widget` does
    // (`DictSource::Impl`). So a new impl on `Widget` must re-check the call site
    // (it holds a `Widget`) but leave the generic function's module warm:
    // `describe` (trait + generic fn) depends on no module that declares `Widget`,
    // so `Widget` is not in its dispatch scope. This is the soundness of the
    // call-site rule — the generic function never hard-links `Describe for Widget`.
    let describe = "pub unique(BEEF0000-0000-4000-8000-000000000010) trait Describe { fn describe(self): Number; }\n\
                    pub fn use_describe<T: Describe>(x: T): Number { x.describe() }\n";
    let widget = "use pkg::describe::Describe;\n\
                  pub unique(BEEF0000-0000-4000-8000-000000000011) struct Widget { x: Number }\n\
                  impl Describe for Widget { fn describe(self): Number { self.x } }\n";
    let main = "use pkg::widget::Widget;\nuse pkg::describe::use_describe;\n\
                pub fn run(): Number { use_describe(Widget { x: 5 }) }\n";
    let base: &[(&str, &str)] = &[
        ("describe.ab", describe),
        ("widget.ab", widget),
        ("main.ab", main),
    ];
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), base);
    build_and_persist(dir.path());

    // Add an orphan `Eq for Widget` in a brand-new module. It changes `Widget`'s
    // impl surface without touching any existing module's source.
    let more = "use pkg::widget::Widget;\n\
                impl Eq for Widget { fn eq(self, o: Widget): Bool { self.x == o.x } }\n";
    fs::write(dir.path().join("src/more.ab"), more).expect("write more");
    let edited: &[(&str, &str)] = &[
        ("describe.ab", describe),
        ("widget.ab", widget),
        ("main.ab", main),
        ("more.ab", more),
    ];

    let (warm, seen) = build_capturing(dir.path());
    if !verify_mode() {
        assert_eq!(
            seen.get("main"),
            Some(&false),
            "the call site instantiates `Describe` at concrete `Widget`, so it re-checks"
        );
        assert_eq!(
            seen.get("describe"),
            Some(&true),
            "the generic function's module holds no Widget (dictionary passing): warm"
        );
    }
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
    // Only D's helper re-checks+compiles (its AST moved). `impls` (links `base`)
    // and `main` (links the dispatch symbol) fail link validation but keep their
    // cache keys, so both take the relink fast path — no re-check, no codegen —
    // while widget + describe stay full hits.
    if !verify_mode() {
        assert_eq!(
            warm.modules_compiled, 1,
            "only helper re-checks+compiles; the rest of the chain relinks"
        );
        assert_eq!(
            warm.modules_relinked, 2,
            "impls + main relink through the moved base/dispatch hashes"
        );
    }
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
    if !verify_mode() {
        assert_eq!(
            healed.modules_compiled, 0,
            "store self-healed on re-persist"
        );
    }
}

#[test]
fn verify_mode_recompiles_and_agrees_on_a_multi_module_build() {
    // The oracle is env-driven, and env vars are process-global — unsafe to
    // mutate from a parallel in-process test — so drive it through a real
    // `ambient run` subprocess (its own process, its own env). A whole-package
    // build persists the snapshot first (`ambient run` is lazy and read-only,
    // so it never writes one), then a warm run under `AMBIENT_CACHE_VERIFY=1`
    // hits every module and proves the hit path agrees with a fresh compile
    // (the standing under-invalidation detector would panic → nonzero exit on
    // any mismatch). The entry reaches all three modules, so the lazy run
    // exercises every one.
    let files: &[(&str, &str)] = &[
        ("a.ab", "pub fn a(): Number { 1 }\n"),
        ("b.ab", "use pkg::a::a;\npub fn b(): Number { a() + 1 }\n"),
        ("main.ab", "use pkg::b::b;\npub fn run(): Number { b() }\n"),
    ];
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), files);
    // Persist a whole-package snapshot so the verify run below has hits to check.
    let cold = build_and_persist(dir.path());
    assert_eq!(cold.module_count, 3, "the package has three modules");

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

#[test]
fn verify_mode_covers_the_relink_fast_path_on_a_body_only_edit() {
    // The relink fast path must be under the standing under-invalidation oracle:
    // a dependency's body-only edit makes the dependents relink, and under
    // `AMBIENT_CACHE_VERIFY=1` the build recompiles each relinked module fully
    // and asserts the relink output is byte-identical (a mismatch panics →
    // nonzero exit). Driven through a real subprocess so the env var is
    // process-local.
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
    // Cold build persists a snapshot (objects + prelink blobs) to relink against.
    build_and_persist(dir.path());

    // Body-only edit of the leaf: dependents will relink on the warm build.
    fs::write(
        dir.path().join("src/leaf.ab"),
        "pub fn base(): Number { 2 }\n",
    )
    .expect("edit");

    // Warm build under the verify oracle: relinked dependents are recompiled and
    // compared. Any stale relink panics inside the build → nonzero exit.
    let out = Command::new(env!("CARGO_BIN_EXE_ambient"))
        .arg("run")
        .arg(dir.path())
        .env("AMBIENT_CACHE_VERIFY", "1")
        .output()
        .expect("spawn ambient run");
    assert!(
        out.status.success(),
        "relink under verify mode disagreed with a fresh compile: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The edited program still produces the right answer (2 + 2 = 4).
    assert!(
        String::from_utf8_lossy(&out.stdout).contains('4'),
        "run must yield the edited value: {}",
        String::from_utf8_lossy(&out.stdout)
    );
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
    if !verify_mode() {
        assert_eq!(warm.modules_compiled, 0, "unchanged rebuild is a full hit");
    }
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
fn unrelated_impl_body_edit_leaves_unrelated_modules_cached_warm() {
    // Phase 5 step 2 (build cache): the dispatch surface is body-free, so an
    // impl *body* edit no longer moves every module's cache key. `shapes`
    // recompiles (its source changed) and `consumer` recompiles (it links the
    // moved dispatch symbol → link validation), but `unrelated` — which does
    // not reference the type — stays a warm hit. Under the old body-bearing
    // global dispatch hash the whole package recompiled.
    let base: &[(&str, &str)] = &[
        (
            "shapes.ab",
            "pub unique(D1D1D1D1-0000-0000-0000-000000000001) struct P { x: Number }\n\
             impl P { fn get(self): Number { self.x } }\n",
        ),
        (
            "main.ab",
            "use pkg::shapes::P;\npub fn run(): Number { P { x: 1 }.get() }\n",
        ),
        ("unrelated.ab", "pub fn f(): Number { 2 }\n"),
    ];
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), base);
    build_and_persist(dir.path());

    let edited: &[(&str, &str)] = &[
        (
            "shapes.ab",
            "pub unique(D1D1D1D1-0000-0000-0000-000000000001) struct P { x: Number }\n\
             impl P { fn get(self): Number { self.x + 0 } }\n",
        ),
        base[1],
        base[2],
    ];
    fs::write(dir.path().join("src/shapes.ab"), edited[0].1).expect("edit");

    let warm = build_and_persist(dir.path());
    // An impl *body* enters the defining module's interface hash (a consumer
    // may dispatch the method by type without importing it), so `shapes`'
    // interface hash moves and `main`'s cache key with it — `main` re-checks
    // rather than relinks. The relink fast path is reserved for *plain*
    // function body edits, which are absent from the interface hash.
    if !verify_mode() {
        assert_eq!(
            warm.modules_compiled, 2,
            "shapes (source) + main (interface-hash key move) recompile; unrelated stays cached, got {}",
            warm.modules_compiled
        );
        assert_eq!(
            warm.modules_relinked, 0,
            "an impl-body edit moves the dependent's key, so it recompiles, not relinks"
        );
    }
    assert_warm_equals_cold(&warm, edited);
}

#[test]
fn ability_method_add_spares_non_consumers() {
    // Abilities carry no dispatch-key input: the dependency channel covers them,
    // because every performer/handler *names* the ability (a resolve-dep edge to
    // its declaring module). Adding a method to `Counter` moves `effects`'
    // interface hash, so `performer` (performs `Counter::next`) and `handler`
    // (handles it) recompile; `unrelated` names no ability and stays a warm hit.
    // Under the old build-global ability fold the whole package recompiled.
    let effects = "pub unique(C0DEC0DE-0000-4000-8000-000000000101) ability Counter {\n\
                   \x20   fn next(): Number { 0 }\n}\n";
    let performer = "use pkg::effects::Counter;\n\
                     pub fn run(): Number with Counter { Counter::next!() }\n";
    let handler = "use pkg::effects::Counter;\n\
                   pub fn h(): Number { with { Counter::next() => resume(5) } handle Counter::next!() }\n";
    let base: &[(&str, &str)] = &[
        ("effects.ab", effects),
        ("performer.ab", performer),
        ("handler.ab", handler),
        ("unrelated.ab", "pub fn f(): Number { 2 }\n"),
        ("main.ab", "pub fn run(): Number { 7 }\n"),
    ];
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), base);
    build_and_persist(dir.path());

    let effects_edited = "pub unique(C0DEC0DE-0000-4000-8000-000000000101) ability Counter {\n\
                          \x20   fn next(): Number { 0 }\n\
                          \x20   fn reset(): Number { 1 }\n}\n";
    fs::write(dir.path().join("src/effects.ab"), effects_edited).expect("edit");
    let edited: &[(&str, &str)] = &[
        ("effects.ab", effects_edited),
        ("performer.ab", performer),
        ("handler.ab", handler),
        ("unrelated.ab", "pub fn f(): Number { 2 }\n"),
        ("main.ab", "pub fn run(): Number { 7 }\n"),
    ];

    let (warm, seen) = build_capturing(dir.path());
    if !verify_mode() {
        assert_eq!(
            seen.get("performer"),
            Some(&false),
            "performer names Counter: recompiles"
        );
        assert_eq!(
            seen.get("handler"),
            Some(&false),
            "handler names Counter: recompiles"
        );
        assert_eq!(
            seen.get("unrelated"),
            Some(&true),
            "unrelated names no ability: warm"
        );
    }
    assert_warm_equals_cold(&warm, edited);
}

#[test]
fn ability_default_body_edit_spares_non_consumers() {
    // A default-impl *body* edit behaves exactly like an impl-body edit: the body
    // is retained in the defining module's full interface hash, so `performer`
    // (imports `effects`) recompiles, while `unrelated` — naming no ability —
    // stays a warm hit. The build-global ability shape was already body-free, so
    // this behavior is unchanged from before abilities left the cache key.
    let effects = "pub unique(C0DEC0DE-0000-4000-8000-000000000102) ability Counter {\n\
                   \x20   fn next(): Number { 0 }\n}\n";
    let performer = "use pkg::effects::Counter;\n\
                     pub fn run(): Number with Counter { Counter::next!() }\n";
    let base: &[(&str, &str)] = &[
        ("effects.ab", effects),
        ("performer.ab", performer),
        ("unrelated.ab", "pub fn f(): Number { 2 }\n"),
        ("main.ab", "pub fn run(): Number { 7 }\n"),
    ];
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), base);
    build_and_persist(dir.path());

    let effects_edited = "pub unique(C0DEC0DE-0000-4000-8000-000000000102) ability Counter {\n\
                          \x20   fn next(): Number { 1 }\n}\n";
    fs::write(dir.path().join("src/effects.ab"), effects_edited).expect("edit");
    let edited: &[(&str, &str)] = &[
        ("effects.ab", effects_edited),
        ("performer.ab", performer),
        ("unrelated.ab", "pub fn f(): Number { 2 }\n"),
        ("main.ab", "pub fn run(): Number { 7 }\n"),
    ];

    let (warm, seen) = build_capturing(dir.path());
    if !verify_mode() {
        assert_eq!(
            seen.get("performer"),
            Some(&false),
            "performer imports effects: recompiles on body edit"
        );
        assert_eq!(
            seen.get("unrelated"),
            Some(&true),
            "unrelated names no ability: warm"
        );
    }
    assert_warm_equals_cold(&warm, edited);
}

#[test]
fn duplicate_impl_in_unrelated_module_still_errors_on_a_warm_build() {
    // Phase 5 step 2: narrowing the dispatch surface must NOT weaken
    // coherence. Coherence keys on the `(trait, type)` identity, which stays
    // in the body-free surface, so *adding* a second impl of the same trait
    // for the same type — even in a module unrelated to the rest — moves the
    // coherence surface, the new module re-checks, and the duplicate is
    // reported. The warm build must fail.
    let base: &[(&str, &str)] = &[
        (
            "defs.ab",
            "pub unique(E0E0E0E0-0000-0000-0000-000000000001) struct W { n: Number }\n\
             pub unique(E0E0E0E0-0000-0000-0000-000000000002) trait Named { fn name(self): Number; }\n",
        ),
        (
            "impl_one.ab",
            "use pkg::defs::W;\nuse pkg::defs::Named;\n\
             impl Named for W { fn name(self): Number { self.n } }\n",
        ),
        ("main.ab", "pub fn run(): Number { 7 }\n"),
    ];
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), base);
    // Cold build succeeds and persists a snapshot the warm build can hit.
    build_and_persist(dir.path());

    // Add a *duplicate* impl in a brand-new, otherwise-unrelated module.
    fs::write(
        dir.path().join("src/impl_two.ab"),
        "use pkg::defs::W;\nuse pkg::defs::Named;\n\
         impl Named for W { fn name(self): Number { self.n + 1 } }\n",
    )
    .expect("write duplicate impl");

    let stubs = ambient_platform::stub_natives();
    let result = build_package(
        dir.path(),
        parse_source,
        &BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            store_path: Some(store_path(dir.path())),
            ..Default::default()
        },
    );
    let msg = match result {
        Ok(_) => panic!("a duplicate impl must fail the warm build"),
        Err(e) => e.to_string(),
    };
    assert!(
        msg.contains("duplicate") || msg.contains("Named"),
        "warm build must surface the coherence error, got: {msg}"
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

// ── `ambient compile` warm builds (feeds and consumes the cache) ────────────

/// The artifact-pack bytes `ambient compile -o` writes for a build: the same
/// encoding `compile_package_cmd` emits, so a byte comparison here proves the
/// warm and cold artifacts are identical.
fn artifact_pack_bytes(result: &BuildResult) -> Vec<u8> {
    result.compiled.to_pack().encode()
}

#[test]
fn compile_wiring_second_build_is_a_full_warm_hit() {
    // In-process mirror of `compile_package_cmd`'s wiring (build reading the
    // package store + persist). The second build must be a full warm hit and
    // its artifact pack byte-identical to the cold one — the guarantee the
    // command depends on.
    let files: &[(&str, &str)] = &[
        ("util.ab", "pub fn helper(): Number { 41 }\n"),
        (
            "main.ab",
            "use pkg::util::helper;\npub fn run(): Number { helper() + 1 }\n",
        ),
    ];
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), files);

    let cold = build_and_persist(dir.path());
    assert!(cold.modules_compiled > 0, "first compile builds everything");

    let warm = build_and_persist(dir.path());
    if !verify_mode() {
        assert_eq!(
            warm.modules_compiled, 0,
            "an unchanged second compile must hit every module"
        );
    }
    assert_eq!(
        artifact_pack_bytes(&warm),
        artifact_pack_bytes(&cold),
        "the warm artifact pack must be byte-identical to the cold one"
    );
}

#[test]
fn ambient_compile_twice_is_warm_and_byte_identical() {
    // Drive the real `ambient compile` command twice (own process, own env) so
    // the whole command wiring — warm read + persist + artifact write — is
    // exercised. `AMBIENT_CACHE_VERIFY=1` turns the second (warm) build into
    // the under-invalidation oracle: any stale hit panics → nonzero exit. The
    // two `-o` artifacts must be byte-identical.
    let files: &[(&str, &str)] = &[
        ("a.ab", "pub fn a(): Number { 1 }\n"),
        ("b.ab", "use pkg::a::a;\npub fn b(): Number { a() + 1 }\n"),
        ("main.ab", "use pkg::b::b;\npub fn run(): Number { b() }\n"),
    ];
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), files);

    let cold_out = dir.path().join("cold.ambient");
    let warm_out = dir.path().join("warm.ambient");

    for out in [&cold_out, &warm_out] {
        let status = Command::new(env!("CARGO_BIN_EXE_ambient"))
            .arg("compile")
            .arg(dir.path())
            .arg("-o")
            .arg(out)
            .env("AMBIENT_CACHE_VERIFY", "1")
            .output()
            .expect("spawn ambient compile");
        assert!(
            status.status.success(),
            "ambient compile failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    let cold_bytes = fs::read(&cold_out).expect("read cold artifact");
    let warm_bytes = fs::read(&warm_out).expect("read warm artifact");
    assert_eq!(
        cold_bytes, warm_bytes,
        "warm and cold `ambient compile` artifacts must be byte-identical"
    );
}

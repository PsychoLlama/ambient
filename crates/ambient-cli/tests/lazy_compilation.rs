//! Coverage of lazy (reachability-restricted) `ambient run` builds.
//!
//! `ambient run` compiles only the modules reachable from its entry point;
//! `ambient check`/`compile`/`dev` stay whole-package. These tests pin the
//! three correctness properties of that split:
//!
//! 1. **Byte-identity** — a reached module's objects are identical whether the
//!    build was lazy or whole-package (content addressing must not depend on
//!    which *other* modules compiled).
//! 2. **Dispatch soundness** — a program whose behavior depends on a trait impl
//!    defined in a module the entry never imports still builds and runs, because
//!    the impl module is pulled into the reachable set through its target type.
//! 3. **Diagnostics policy** — a type error in an unreachable module does not
//!    fail `ambient run` (that module is never checked), but `ambient check`
//!    still reports it.

use std::path::Path;
use std::process::Command;

use ambient_engine::build::{BuildOptions, BuildResult, CacheMode, build_package};
use tempfile::TempDir;

mod common;
use common::{parse_source, write_pkg_named};

/// The package name these lazy tests build under; the reachable-module
/// assertions below spell it (`workspace::lazy_pkg::…`).
const PKG: &str = "lazy_pkg";

fn write_pkg(dir: &Path, files: &[(&str, &str)]) {
    write_pkg_named(dir, PKG, files);
}

/// Build in a fresh package (cold, no store), either whole-package
/// (`entry: None`) or lazily restricted to the entry's reachable closure.
fn build(files: &[(&str, &str)], entry: Option<&str>) -> Result<BuildResult, String> {
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), files);
    let stubs = ambient_platform::stub_natives();
    build_package(
        dir.path(),
        parse_source,
        &BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            cache: CacheMode::Off,
            entry,
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())
}

/// The package (non-builtin) module ids a build produced objects for.
fn package_modules(result: &BuildResult) -> Vec<String> {
    let mut ids: Vec<String> = result
        .module_outputs
        .keys()
        .filter(|k| k.starts_with("workspace::"))
        .cloned()
        .collect();
    ids.sort();
    ids
}

/// The `ambient` CLI binary under test.
fn ambient_bin() -> &'static str {
    env!("CARGO_BIN_EXE_ambient")
}

#[test]
fn lazy_run_skips_unreachable_modules_and_matches_full() {
    // `main` uses `lib`; `unused` is an unrelated module nobody imports.
    let files: &[(&str, &str)] = &[
        ("lib.ab", "pub fn answer(): Number { 42 }\n"),
        (
            "main.ab",
            "use pkg::lib::answer;\npub fn run(): Number { answer() }\n",
        ),
        (
            "unused.ab",
            "pub fn dead(): Number { 99 }\npub fn also_dead(): Number { dead() }\n",
        ),
    ];

    let full = build(files, None).expect("full build");
    let lazy = build(files, Some("run")).expect("lazy build");

    // The whole package has three modules; the lazy build reaches only two.
    assert_eq!(package_modules(&full).len(), 3);
    let lazy_mods = package_modules(&lazy);
    assert_eq!(
        lazy_mods,
        vec![
            "workspace::lazy_pkg::lib".to_string(),
            "workspace::lazy_pkg::main".to_string(),
        ],
        "the unreachable `unused` module must be pruned"
    );

    // Every reached module's objects are byte-identical to the full build's.
    for id in &lazy_mods {
        assert_eq!(
            lazy.module_outputs[id].objects, full.module_outputs[id].objects,
            "reached module `{id}` must produce identical objects lazily vs. whole-package"
        );
    }
}

#[test]
fn lazy_run_pulls_in_a_trait_impl_the_entry_never_imports() {
    // The orphan-impl chain: `main` dispatches `w.describe()` on a `Widget` but
    // imports neither `impls` (where `impl Describe for Widget` lives) nor
    // `helper` (which the impl calls). Reachability must still pull both in
    // through `Widget`'s module, or the dispatch symbol would be missing.
    let files: &[(&str, &str)] = &[
        (
            "widget.ab",
            "pub unique(DEADBEEF-0000-0000-0000-000000000001) struct Widget { size: Number }\n",
        ),
        (
            "describe.ab",
            "pub unique(DEADBEEF-0000-0000-0000-000000000002) trait Describe { fn describe(self): Number; }\n",
        ),
        ("helper.ab", "pub fn base(): Number { 10 }\n"),
        (
            "impls.ab",
            "use pkg::widget::Widget;\n\
             use pkg::describe::Describe;\n\
             use pkg::helper::base;\n\
             impl Describe for Widget { fn describe(self): Number { base() + self.size } }\n",
        ),
        (
            "main.ab",
            "use pkg::widget::Widget;\n\
             use pkg::describe::Describe;\n\
             pub fn run(): Number { let w = Widget { size: 5 }; w.describe() }\n",
        ),
    ];

    let full = build(files, None).expect("full build");
    let lazy = build(files, Some("run")).expect("lazy build must succeed with the impl in reach");

    // All five modules are needed and reached.
    assert_eq!(package_modules(&lazy), package_modules(&full));

    // The dispatch symbol the impl defines is present, and the entry compiled.
    assert!(
        lazy.compiled
            .function_names
            .keys()
            .any(|n| n.contains("::") && n.contains("describe")),
        "the `Describe::describe` dispatch symbol must be compiled"
    );
    assert!(lazy.compiled.entry_point.is_some(), "entry point captured");

    // And every reached object is byte-identical to the whole-package build.
    for id in package_modules(&lazy) {
        assert_eq!(
            lazy.module_outputs[&id].objects,
            full.module_outputs[&id].objects
        );
    }
}

#[test]
fn orphan_impl_links_when_impl_module_sorts_after_the_dispatcher() {
    // The orphan impl lives in `zebra.ab`; the dispatch site is in `main.ab`.
    // `main` never imports `zebra`, and `zebra` sorts *after* `main`
    // alphabetically — so a resolve-deps-only compile order puts `main` first
    // and its `w.describe()` fails to link against `zebra`'s dispatch symbol.
    // The structural dispatch-ordering edge must force `zebra` to compile first.
    let files: &[(&str, &str)] = &[
        (
            "common.ab",
            "pub unique(DEADBEEF-0000-0000-0000-0000000000AA) struct Widget { size: Number }\n\
             pub unique(DEADBEEF-0000-0000-0000-0000000000AB) trait Describe { fn describe(self): Number; }\n",
        ),
        (
            "main.ab",
            "use pkg::common::Widget;\n\
             use pkg::common::Describe;\n\
             pub fn run(): Number { let w = Widget { size: 5 }; w.describe() }\n",
        ),
        (
            "zebra.ab",
            "use pkg::common::Widget;\n\
             use pkg::common::Describe;\n\
             impl Describe for Widget { fn describe(self): Number { 10 + self.size } }\n",
        ),
    ];

    // Whole-package build: must link `apple` against `zebra`'s orphan impl.
    let full = build(files, None).expect("full build must link the orphan impl");
    assert_eq!(package_modules(&full).len(), 3);

    // Lazy build reaches all three (the impl is pulled in via `Widget`).
    let lazy = build(files, Some("run")).expect("lazy build must link the orphan impl");
    assert_eq!(package_modules(&lazy), package_modules(&full));
    for id in package_modules(&lazy) {
        assert_eq!(
            lazy.module_outputs[&id].objects,
            full.module_outputs[&id].objects
        );
    }
}

#[test]
fn self_orphan_dispatch_links_when_impl_module_sorts_after_the_type() {
    // The *self-orphan* case: `common` both declares `Widget`/`Describe` AND
    // dispatches on `Widget` (`poke` calls `Widget{..}.describe()`), while the
    // `impl Describe for Widget` lives in later-sorting `zebra`. Compile order
    // therefore needs `common -> zebra` (so `zebra`'s dispatch symbol exists
    // when `common` links). That edge is acyclic only because `zebra -> common`
    // is a `use`/type-target edge (check-order-only, not a link dep): basing the
    // order on `link_deps` drops it, so `common -> zebra` survives. Before that,
    // the build failed with `undefined function: <uuid>::describe`.
    let files: &[(&str, &str)] = &[
        (
            "common.ab",
            "pub unique(DEADBEEF-0000-0000-0000-0000000000BA) struct Widget { size: Number }\n\
             pub unique(DEADBEEF-0000-0000-0000-0000000000BB) trait Describe { fn describe(self): Number; }\n\
             pub fn poke(): Number { Widget { size: 5 }.describe() }\n",
        ),
        (
            "main.ab",
            "use pkg::common::poke;\n\
             pub fn run(): Number { poke() }\n",
        ),
        (
            "zebra.ab",
            "use pkg::common::Widget;\n\
             use pkg::common::Describe;\n\
             impl Describe for Widget { fn describe(self): Number { self.size * 3 } }\n",
        ),
    ];

    // Whole-package build must link `common`'s self-dispatch against `zebra`.
    let full = build(files, None).expect("full build must link the self-orphan dispatch");
    assert_eq!(package_modules(&full).len(), 3);

    // Lazy build reaches all three (`zebra` is pulled in via `Widget`).
    let lazy = build(files, Some("run")).expect("lazy build must link the self-orphan dispatch");
    assert_eq!(package_modules(&lazy), package_modules(&full));
    for id in package_modules(&lazy) {
        assert_eq!(
            lazy.module_outputs[&id].objects, full.module_outputs[&id].objects,
            "reached module `{id}` must produce identical objects lazily vs. whole-package"
        );
    }
}

#[test]
fn ambient_run_executes_self_orphan_dispatch() {
    // End-to-end: `poke` dispatches `Widget{size:5}.describe()` in `common`
    // itself; the impl (`self.size * 3` = 15) lives in later-sorting `zebra`.
    let dir = TempDir::new().expect("temp");
    write_pkg(
        dir.path(),
        &[
            (
                "common.ab",
                "pub unique(CAFE0000-0000-0000-0000-0000000000BA) struct Widget { size: Number }\n\
                 pub unique(CAFE0000-0000-0000-0000-0000000000BB) trait Describe { fn describe(self): Number; }\n\
                 pub fn poke(): Number { Widget { size: 5 }.describe() }\n",
            ),
            (
                "main.ab",
                "use pkg::common::poke;\n\
                 pub fn run(): Number { poke() }\n",
            ),
            (
                "zebra.ab",
                "use pkg::common::Widget;\n\
                 use pkg::common::Describe;\n\
                 impl Describe for Widget { fn describe(self): Number { self.size * 3 } }\n",
            ),
        ],
    );

    let out = Command::new(ambient_bin())
        .arg("run")
        .arg(dir.path())
        .output()
        .expect("spawn ambient run");
    assert!(
        out.status.success(),
        "ambient run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("15"), "expected result 15, got: {stdout}");
}

#[test]
fn ambient_run_self_orphan_dispatch_with_typed_record_in_impl_body() {
    // Regression: the self-orphan dispatch must still link when `zebra`'s impl
    // body itself *constructs* the foreign `Widget` (`Widget { size: 1 }`).
    // Constructing a foreign typed record emits no link artifact (it lowers to a
    // plain `MakeRecord`, discarding the type name), so it must be a CHECK-only
    // dep, not a link dep. If it were misclassified as a link dep, `zebra ->
    // common` would become a link edge, the candidate-skip would drop the needed
    // `common -> zebra` dispatch edge, and the build would fail with `undefined
    // function: <uuid>::describe`.
    //
    // `describe(self)` returns `Widget { size: 1 }.size + self.size * 3`; `poke`
    // dispatches `Widget { size: 5 }.describe()`, so the result is
    // `1 + 5 * 3 = 16`.
    let dir = TempDir::new().expect("temp");
    write_pkg(
        dir.path(),
        &[
            (
                "common.ab",
                "pub unique(CAFE0000-0000-0000-0000-0000000000CA) struct Widget { size: Number }\n\
                 pub unique(CAFE0000-0000-0000-0000-0000000000CB) trait Describe { fn describe(self): Number; }\n\
                 pub fn poke(): Number { Widget { size: 5 }.describe() }\n",
            ),
            (
                "main.ab",
                "use pkg::common::poke;\n\
                 pub fn run(): Number { poke() }\n",
            ),
            (
                "zebra.ab",
                "use pkg::common::Widget;\n\
                 use pkg::common::Describe;\n\
                 impl Describe for Widget { fn describe(self): Number { Widget { size: 1 }.size + self.size * 3 } }\n",
            ),
        ],
    );

    let out = Command::new(ambient_bin())
        .arg("run")
        .arg(dir.path())
        .output()
        .expect("spawn ambient run");
    assert!(
        out.status.success(),
        "ambient run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("16"), "expected result 16, got: {stdout}");
}

#[test]
fn ambient_run_self_orphan_dispatch_with_qualified_typed_record_in_impl_body() {
    // The qualified-spelling twin of the test above: `zebra`'s impl body
    // constructs the foreign record via its fully-qualified path
    // (`pkg::common::Widget { size: 1 }`). Per the `Fqn` invariant, the qualified
    // and bare spellings must classify identically — both CHECK-only — so this
    // resolves through the shared `resolve_path_ref`/`lookup_item` path carrying
    // an explicit type position. Same arithmetic: `1 + 5 * 3 = 16`.
    let dir = TempDir::new().expect("temp");
    write_pkg(
        dir.path(),
        &[
            (
                "common.ab",
                "pub unique(CAFE0000-0000-0000-0000-0000000000DA) struct Widget { size: Number }\n\
                 pub unique(CAFE0000-0000-0000-0000-0000000000DB) trait Describe { fn describe(self): Number; }\n\
                 pub fn poke(): Number { Widget { size: 5 }.describe() }\n",
            ),
            (
                "main.ab",
                "use pkg::common::poke;\n\
                 pub fn run(): Number { poke() }\n",
            ),
            (
                "zebra.ab",
                "use pkg::common::Widget;\n\
                 use pkg::common::Describe;\n\
                 impl Describe for Widget { fn describe(self): Number { pkg::common::Widget { size: 1 }.size + self.size * 3 } }\n",
            ),
        ],
    );

    let out = Command::new(ambient_bin())
        .arg("run")
        .arg(dir.path())
        .output()
        .expect("spawn ambient run");
    assert!(
        out.status.success(),
        "ambient run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("16"), "expected result 16, got: {stdout}");
}

#[test]
fn genuine_dispatch_cycle_still_fails_to_link() {
    // The self-orphan shape, but `zebra`'s impl body now *link*-depends on
    // `common` (`common::helper(...)`), so the structural `common -> zebra`
    // dispatch edge meets a real `zebra -> common` link dep: a genuine cycle
    // single-pass linking cannot satisfy. Per-edge acyclic augmentation adds
    // the needed `common -> zebra` edge only if it keeps the graph acyclic —
    // but `zebra -> common` is already in the `link_deps` base, so that edge
    // would close a cycle and is individually skipped. `common` then stays
    // ordered before `zebra`, so its self-dispatch fails to link exactly as it
    // did before — the deliberate, correct outcome for a truly cyclic dispatch
    // dependency.
    let files: &[(&str, &str)] = &[
        (
            "common.ab",
            "pub unique(DEADBEEF-0000-0000-0000-0000000000CA) struct Widget { size: Number }\n\
             pub unique(DEADBEEF-0000-0000-0000-0000000000CB) trait Describe { fn describe(self): Number; }\n\
             pub fn helper(n: Number): Number { n + 1 }\n\
             pub fn poke(): Number { Widget { size: 5 }.describe() }\n",
        ),
        (
            "main.ab",
            "use pkg::common::poke;\n\
             pub fn run(): Number { poke() }\n",
        ),
        (
            "zebra.ab",
            "use pkg::common::Widget;\n\
             use pkg::common::Describe;\n\
             use pkg::common::helper;\n\
             impl Describe for Widget { fn describe(self): Number { helper(self.size) } }\n",
        ),
    ];

    let err = match build(files, None) {
        Ok(_) => panic!("a genuine dispatch cycle must fail to link"),
        Err(e) => e,
    };
    assert!(
        err.contains("describe") || err.to_lowercase().contains("undefined"),
        "expected an undefined-dispatch-symbol link error, got: {err}"
    );
}

/// The regression package: a spurious dispatch 2-cycle in one cluster
/// (`gm`/`gc`/`gd`) must not poison an unrelated, perfectly satisfiable orphan
/// dispatch in the same package (`common`/`zebra`/`main`).
///
/// The cluster manufactures a 2-cycle among structural edges: `gm` declares
/// `Bee`+`Wye`+`zero()`; `gc` declares `Aye`+`Exx` and holds `impl Wye for Bee`
/// (type-only dep `gc -> gm`, pure body — no link dep); `gd` holds
/// `impl Exx for Aye` (type-only dep `gd -> gc`) and *calls* `gm::zero()` (link
/// dep `gd -> gm`). The structural dispatch edges are `gm -> gc`, `gd -> gc`
/// (dispatchers of `Bee`) and `gc -> gd` (dispatcher of `Aye`); the pair
/// `gc -> gd` + `gd -> gc` is a cycle. The old all-or-nothing guard discarded
/// *every* structural edge on any cycle, dropping the unrelated `main -> zebra`
/// edge the orphan needs (`main` dispatches `Widget.describe()`, `impl` lives in
/// later-sorting `zebra`), so the whole-package build failed to link. The
/// per-edge acyclic augmentation drops only the specific cycle-closing edge, so
/// `main -> zebra` survives and the program links and prints 15.
const SPURIOUS_CYCLE_PKG: &[(&str, &str)] = &[
    (
        "gm.ab",
        "pub unique(DEADBEEF-0000-0000-0000-0000000000E0) struct Bee { n: Number }\n\
         pub unique(DEADBEEF-0000-0000-0000-0000000000E1) trait Wye { fn wye(self): Number; }\n\
         pub fn zero(): Number { 0 }\n",
    ),
    (
        "gc.ab",
        "use pkg::gm::Bee;\n\
         use pkg::gm::Wye;\n\
         pub unique(DEADBEEF-0000-0000-0000-0000000000E2) struct Aye { n: Number }\n\
         pub unique(DEADBEEF-0000-0000-0000-0000000000E3) trait Exx { fn exx(self): Number; }\n\
         impl Wye for Bee { fn wye(self): Number { self.n } }\n",
    ),
    (
        "gd.ab",
        "use pkg::gc::Aye;\n\
         use pkg::gc::Exx;\n\
         use pkg::gm::zero;\n\
         impl Exx for Aye { fn exx(self): Number { self.n } }\n\
         pub fn g(): Number { zero() }\n",
    ),
    (
        "common.ab",
        "pub unique(DEADBEEF-0000-0000-0000-0000000000EA) struct Widget { size: Number }\n\
         pub unique(DEADBEEF-0000-0000-0000-0000000000EB) trait Describe { fn describe(self): Number; }\n",
    ),
    (
        "zebra.ab",
        "use pkg::common::Widget;\n\
         use pkg::common::Describe;\n\
         impl Describe for Widget { fn describe(self): Number { 10 + self.size } }\n",
    ),
    (
        "main.ab",
        "use pkg::common::Widget;\n\
         use pkg::common::Describe;\n\
         pub fn run(): Number { let w = Widget { size: 5 }; w.describe() }\n",
    ),
];

#[test]
fn spurious_dispatch_cycle_does_not_poison_an_unrelated_orphan() {
    // Whole-package build must link the `main -> zebra` orphan despite the
    // `gc`/`gd` cluster's spurious structural cycle. This FAILS before the
    // per-edge augmentation fix (the wholesale fallback drops `main -> zebra`).
    let full = build(SPURIOUS_CYCLE_PKG, None)
        .expect("whole-package build must link the unrelated orphan despite the spurious cycle");
    assert_eq!(package_modules(&full).len(), 6);

    // And it runs, printing 15 (10 + size 5), proving the orphan dispatch linked.
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), SPURIOUS_CYCLE_PKG);
    let out = Command::new(ambient_bin())
        .arg("run")
        .arg(dir.path())
        .output()
        .expect("spawn ambient run");
    assert!(
        out.status.success(),
        "ambient run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("15"), "expected result 15, got: {stdout}");
}

#[test]
fn lazy_run_is_not_poisoned_by_an_unreachable_spurious_cycle() {
    // The lazy entry (`run` in `main`) reaches only `main`/`common`/`zebra`; the
    // `gm`/`gc`/`gd` cluster (with its spurious structural cycle) is unreachable
    // and must not affect the reachable part's ordering. The ordering graph is
    // computed over *all* modules before the lazy filter, so an unreachable
    // poisoner would still corrupt the reachable order under the old guard.
    let lazy = build(SPURIOUS_CYCLE_PKG, Some("run"))
        .expect("lazy build must link the orphan; the unreachable cluster must not poison it");
    assert_eq!(
        package_modules(&lazy),
        vec![
            "workspace::lazy_pkg::common".to_string(),
            "workspace::lazy_pkg::main".to_string(),
            "workspace::lazy_pkg::zebra".to_string(),
        ],
        "only the orphan cluster is reached; the spurious-cycle cluster is pruned"
    );

    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), SPURIOUS_CYCLE_PKG);
    let out = Command::new(ambient_bin())
        .arg("run")
        .arg(dir.path())
        .output()
        .expect("spawn ambient run");
    assert!(
        out.status.success(),
        "ambient run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("15"));
}

#[test]
fn ambient_run_executes_orphan_impl_module_sorted_after_dispatcher() {
    // End-to-end: the run result (15 = 10 + size 5) proves the dispatch reached
    // `zebra`'s impl even though `zebra` sorts after the dispatching `main`.
    let dir = TempDir::new().expect("temp");
    write_pkg(
        dir.path(),
        &[
            (
                "common.ab",
                "pub unique(CAFE0000-0000-0000-0000-0000000000AA) struct Widget { size: Number }\n\
                 pub unique(CAFE0000-0000-0000-0000-0000000000AB) trait Describe { fn describe(self): Number; }\n",
            ),
            (
                "main.ab",
                "use pkg::common::Widget;\n\
                 use pkg::common::Describe;\n\
                 pub fn run(): Number { let w = Widget { size: 5 }; w.describe() }\n",
            ),
            (
                "zebra.ab",
                "use pkg::common::Widget;\n\
                 use pkg::common::Describe;\n\
                 impl Describe for Widget { fn describe(self): Number { 10 + self.size } }\n",
            ),
        ],
    );

    let out = Command::new(ambient_bin())
        .arg("run")
        .arg(dir.path())
        .output()
        .expect("spawn ambient run");
    assert!(
        out.status.success(),
        "ambient run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("15"), "expected result 15, got: {stdout}");
}

#[test]
fn lazy_run_ignores_a_type_error_in_an_unreachable_module() {
    // `main` is valid; `broken` (imported by nobody) has a type error.
    let files: &[(&str, &str)] = &[
        ("main.ab", "pub fn run(): Number { 7 }\n"),
        ("broken.ab", "pub fn oops(): Number { \"not a number\" }\n"),
    ];

    // Whole-package build checks `broken` and fails.
    assert!(
        build(files, None).is_err(),
        "a whole-package build must surface the type error"
    );

    // Lazy build never checks `broken`, so it succeeds.
    let lazy = build(files, Some("run")).expect("lazy build ignores the unreachable error");
    assert_eq!(
        package_modules(&lazy),
        vec!["workspace::lazy_pkg::main".to_string()]
    );
}

#[test]
fn ambient_run_executes_a_program_with_an_orphan_trait_impl() {
    // The behavior test: the run result (15 = base 10 + size 5) proves the
    // dispatch actually reached the impl in the never-imported module.
    let dir = TempDir::new().expect("temp");
    write_pkg(
        dir.path(),
        &[
            (
                "widget.ab",
                "pub unique(CAFE0000-0000-0000-0000-000000000001) struct Widget { size: Number }\n",
            ),
            (
                "describe.ab",
                "pub unique(CAFE0000-0000-0000-0000-000000000002) trait Describe { fn describe(self): Number; }\n",
            ),
            ("helper.ab", "pub fn base(): Number { 10 }\n"),
            (
                "impls.ab",
                "use pkg::widget::Widget;\n\
                 use pkg::describe::Describe;\n\
                 use pkg::helper::base;\n\
                 impl Describe for Widget { fn describe(self): Number { base() + self.size } }\n",
            ),
            (
                "main.ab",
                "use pkg::widget::Widget;\n\
                 use pkg::describe::Describe;\n\
                 pub fn run(): Number { let w = Widget { size: 5 }; w.describe() }\n",
            ),
        ],
    );

    let out = Command::new(ambient_bin())
        .arg("run")
        .arg(dir.path())
        .output()
        .expect("spawn ambient run");
    assert!(
        out.status.success(),
        "ambient run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("15"), "expected result 15, got: {stdout}");
}

#[test]
fn ambient_run_ignores_unreachable_errors_but_check_reports_them() {
    let dir = TempDir::new().expect("temp");
    write_pkg(
        dir.path(),
        &[
            ("main.ab", "pub fn run(): Number { 7 }\n"),
            ("broken.ab", "pub fn oops(): Number { \"not a number\" }\n"),
        ],
    );

    // `ambient run` reaches only `main`; the error in `broken` is invisible.
    let run = Command::new(ambient_bin())
        .arg("run")
        .arg(dir.path())
        .output()
        .expect("spawn ambient run");
    assert!(
        run.status.success(),
        "ambient run must ignore the unreachable module: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(String::from_utf8_lossy(&run.stdout).contains('7'));

    // `ambient check` is whole-package and must report the error.
    let check = Command::new(ambient_bin())
        .arg("check")
        .arg(dir.path())
        .output()
        .expect("spawn ambient check");
    assert!(
        !check.status.success(),
        "ambient check must fail on the type error"
    );
}

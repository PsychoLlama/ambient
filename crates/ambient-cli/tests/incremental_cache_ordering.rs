//! Incremental-cache coverage for the `link_deps`-based compile ordering
//! (phase 3 of the self-orphan dispatch fix).
//!
//! These live apart from `incremental_cache.rs` only because that file is at
//! its file-size budget. They assert the same warm==cold byte-identity property
//! for the two module populations the reorder affects: a *self-orphan* dispatch
//! (a module dispatching on its own type whose impl sorts after it) and a
//! *type-only* cross-module import (a `deps` edge that is not a `link_deps`
//! edge, so it no longer constrains compile order).

use std::fs;
use std::path::Path;

use ambient_engine::ast::Module;
use ambient_engine::build::{BuildOptions, BuildResult, ParseFailure};
use ambient_engine::disk_store::BuildManifest;
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

/// Build reading the package's own store, then persist objects + snapshot so the
/// next build can hit — the same wiring `ambient run`/`compile` use.
fn build_and_persist(dir: &Path) -> BuildResult {
    let stubs = ambient_platform::stub_natives();
    let built = ambient_engine::build::build_and_persist(
        dir,
        parse_source,
        BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            ..Default::default()
        },
    )
    .expect("build succeeds");
    built.persisted.expect("persist build");
    built.result
}

/// The canonical cold manifest for a source: build in a fresh, empty-store
/// package so everything compiles.
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

/// `true` under the `AMBIENT_CACHE_VERIFY=1` recompile-and-compare oracle, which
/// recompiles every module so the exact `modules_compiled` counts don't hold.
fn verify_mode() -> bool {
    std::env::var("AMBIENT_CACHE_VERIFY").is_ok_and(|v| v.eq_ignore_ascii_case("1"))
}

#[test]
fn self_orphan_dispatch_warm_equals_cold() {
    // The self-orphan case under the incremental cache: `common` dispatches on
    // its own `Widget` while the `impl` lives in later-sorting `zebra`. The
    // compile-ordering pass (now based on `link_deps`) puts `zebra` first so the
    // dispatch symbol links. A warm rebuild must be byte-identical to cold —
    // proving the reordered compile order is deterministic across builds.
    let files: &[(&str, &str)] = &[
        (
            "common.ab",
            "pub unique(BADA0000-0000-0000-0000-0000000000BA) struct Widget { size: Number }\n\
             pub unique(BADA0000-0000-0000-0000-0000000000BB) trait Describe { fn describe(self): Number; }\n\
             pub fn poke(): Number { Widget { size: 5 }.describe() }\n",
        ),
        (
            "main.ab",
            "use pkg::common::poke;\npub fn run(): Number { poke() }\n",
        ),
        (
            "zebra.ab",
            "use pkg::common::Widget;\nuse pkg::common::Describe;\n\
             impl Describe for Widget { fn describe(self): Number { self.size * 3 } }\n",
        ),
    ];
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), files);

    let cold = build_and_persist(dir.path());
    assert!(cold.modules_compiled > 0, "first build compiles everything");

    let warm = build_and_persist(dir.path());
    if !verify_mode() {
        assert_eq!(
            warm.modules_compiled, 0,
            "an unchanged self-orphan rebuild must hit every module"
        );
    }
    assert_warm_equals_cold(&warm, files);
}

#[test]
fn type_only_import_warm_equals_cold() {
    // The population the `link_deps` reorder affects: module `a` imports module
    // `b` purely in *type* position (a function-signature annotation and an impl
    // target head) — never a value-position reference. So `a -> b` is in `deps`
    // but not `link_deps`, and the compile order no longer forces `b` before `a`
    // for linking. A warm rebuild must still be byte-identical to cold.
    let files: &[(&str, &str)] = &[
        (
            "b.ab",
            "pub unique(B0000000-0000-0000-0000-00000000000B) struct Tag { v: Number }\n\
             pub unique(B0000000-0000-0000-0000-00000000000C) trait Kind { fn kind(self): Number; }\n",
        ),
        (
            "a.ab",
            // `Tag` appears only as a type annotation, and `Kind`/`Tag` only as
            // impl-header heads — both pure type-position (check-order-only).
            "use pkg::b::Tag;\nuse pkg::b::Kind;\n\
             pub fn size_of(t: Tag): Number { t.v }\n\
             impl Kind for Tag { fn kind(self): Number { self.v } }\n",
        ),
        (
            "main.ab",
            "use pkg::a::size_of;\nuse pkg::b::Tag;\n\
             pub fn run(): Number { size_of(Tag { v: 9 }) }\n",
        ),
    ];
    let dir = TempDir::new().expect("temp");
    write_pkg(dir.path(), files);

    let cold = build_and_persist(dir.path());
    assert!(cold.modules_compiled > 0, "first build compiles everything");

    let warm = build_and_persist(dir.path());
    if !verify_mode() {
        assert_eq!(
            warm.modules_compiled, 0,
            "an unchanged type-only-import rebuild must hit every module"
        );
    }
    assert_warm_equals_cold(&warm, files);
}

//! Incremental-cache coverage for the `link_deps`-based compile ordering
//! (phase 3 of the self-orphan dispatch fix).
//!
//! These live apart from `incremental_cache.rs` only because that file is at
//! its file-size budget. They assert the same warm==cold byte-identity property
//! for the two module populations the reorder affects: a *self-orphan* dispatch
//! (a module dispatching on its own type whose impl sorts after it) and a
//! *type-only* cross-module import (a `deps` edge that is not a `link_deps`
//! edge, so it no longer constrains compile order).

use std::path::Path;

use ambient_engine::build::BuildResult;
use tempfile::TempDir;

mod common;
use common::{build_and_persist, verify_mode, write_pkg_named};

/// The package name these ordering tests build under; a warm build and its cold
/// twin must share it (see [`common`]).
const PKG: &str = "cache_pkg";

fn write_pkg(dir: &Path, files: &[(&str, &str)]) {
    write_pkg_named(dir, PKG, files);
}

fn assert_warm_equals_cold(warm: &BuildResult, files: &[(&str, &str)]) {
    common::assert_warm_equals_cold(PKG, warm, files);
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

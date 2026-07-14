//! Incremental-cache coverage for resolve-pass dependency edges that are not
//! link edges — specifically, that a block-scoped `use` records a dependency
//! exactly like a module-level one, so a warm rebuild re-checks a consumer
//! whose only connection to the changed module is a block `use`.
//!
//! Lives apart from `incremental_cache.rs` only because that file is at its
//! file-size budget (see `incremental_cache_ordering.rs` for the same split).

use std::fs;
use std::path::Path;

use ambient_engine::build::{BuildError, BuildOptions, BuildResult, build_package};
use tempfile::TempDir;

mod common;
use common::{
    build_and_persist, build_capturing, parse_source, store_path, verify_mode, write_pkg_named,
};

/// The package name these tests build under; a warm build and its cold twin
/// must share it (see [`common`]).
const PKG: &str = "cache_pkg";

fn write_pkg(dir: &Path, files: &[(&str, &str)]) {
    write_pkg_named(dir, PKG, files);
}

fn assert_warm_equals_cold(warm: &BuildResult, files: &[(&str, &str)]) {
    common::assert_warm_equals_cold(PKG, warm, files);
}

#[test]
fn block_scoped_use_of_a_type_records_a_dependency_edge() {
    // A block-scoped `use` is a dependency exactly like a module-level one:
    // `bind_block_use` records a check-only `RefPos::Import` edge for the type
    // it binds. Here `consumer`'s *only* connection to `defs` is a block-scoped
    // `use pkg::defs::Widget` referenced purely type-side (a lambda parameter
    // annotation) inside `g`'s body — no value ref into `defs` at all. Without
    // the edge, `consumer`'s cache key would not fold `defs`'s interface hash,
    // and a warm rebuild would serve `consumer` from a stale object compiled
    // against the old `Widget` shape.
    //
    // `defs` exposes `Widget` (a record `{ n: Number }`) that `consumer`'s
    // lambda annotates its parameter with and reads `.n` off of.
    let defs_v1 = "pub unique(C0FFEE00-0000-0000-0000-0000000000AB) struct Widget { n: Number }\n";
    // The type-only consumer: `Widget` appears solely as a lambda-parameter
    // annotation (type position); `item.n` is a field access (no dep). The
    // block `use` is the sole link between `consumer` and `defs`.
    let consumer = "pub fn g(w: Number): Number {\n\
                    \x20   use pkg::defs::Widget;\n\
                    \x20   let describe = (item: Widget) => item.n;\n\
                    \x20   w\n\
                    }\n";
    let main = "pub fn run(): Number { 0 }\n";

    // ── Phase A: a *compatible* interface change to `defs` re-checks the
    // consumer (and only the consumer). Adding an unrelated `pub fn` moves
    // `defs`'s interface hash; `consumer` folds it (via the new block-`use`
    // edge) and re-checks, while `main` — which has no dep on `defs` — stays a
    // warm hit. This is what pins the dep edge: pre-fix, `consumer` had no dep
    // on `defs` and would have stayed a stale warm hit here.
    let dir = TempDir::new().expect("temp");
    write_pkg(
        dir.path(),
        &[
            ("defs.ab", defs_v1),
            ("consumer.ab", consumer),
            ("main.ab", main),
        ],
    );
    build_and_persist(dir.path());

    let defs_v2 = "pub unique(C0FFEE00-0000-0000-0000-0000000000AB) struct Widget { n: Number }\n\
                   pub fn extra(): Number { 1 }\n";
    fs::write(dir.path().join("src/defs.ab"), defs_v2).expect("edit");
    let edited: &[(&str, &str)] = &[
        ("defs.ab", defs_v2),
        ("consumer.ab", consumer),
        ("main.ab", main),
    ];

    let (warm, seen) = build_capturing(dir.path());
    if !verify_mode() {
        assert_eq!(
            seen.get("consumer"),
            Some(&false),
            "consumer depends on defs via a block-scoped use, so a defs interface change re-checks it"
        );
        assert_eq!(
            seen.get("main"),
            Some(&true),
            "main has no dep on defs: it stays a warm hit"
        );
    }
    assert_warm_equals_cold(&warm, edited);

    // ── Phase B: a *breaking* interface change (rename the used type away) must
    // surface the error on a warm rebuild rather than serve `consumer` stale.
    // Fresh package so the warm build starts from a clean cold snapshot.
    let dir = TempDir::new().expect("temp");
    write_pkg(
        dir.path(),
        &[
            ("defs.ab", defs_v1),
            ("consumer.ab", consumer),
            ("main.ab", main),
        ],
    );
    build_and_persist(dir.path());

    // Rename `Widget` → `Gadget`: `consumer`'s block `use pkg::defs::Widget`
    // now names a nonexistent item, so `consumer` must fail to check.
    let defs_renamed =
        "pub unique(C0FFEE00-0000-0000-0000-0000000000AB) struct Gadget { n: Number }\n";
    fs::write(dir.path().join("src/defs.ab"), defs_renamed).expect("edit");

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
    let Err(BuildError::TypeCheck { failures }) = result else {
        panic!("renaming a block-imported type must fail the warm build, not serve consumer stale");
    };
    let msg = failures
        .iter()
        .flat_map(|f| f.errors.iter().map(ToString::to_string))
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        msg.contains("Widget"),
        "the warm build must surface consumer's now-broken block use, got: {msg}"
    );
}

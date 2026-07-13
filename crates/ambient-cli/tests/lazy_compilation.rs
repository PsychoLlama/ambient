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

use std::fs;
use std::path::Path;
use std::process::Command;

use ambient_engine::ast::Module;
use ambient_engine::build::{BuildOptions, BuildResult, CacheMode, ParseFailure, build_package};
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
        "[package]\nname = \"lazy_pkg\"\nversion = \"0.1.0\"\n",
    )
    .expect("manifest");
    let src = dir.join("src");
    for (rel, body) in files {
        let path = src.join(rel);
        fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        fs::write(path, body).expect("write module");
    }
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

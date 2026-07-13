//! End-to-end coverage of build snapshots (incremental-compilation Phase 2):
//! `ambient run` records a crash-safe manifest, `ambient store snapshot`
//! summarizes it, and two builds of identical source produce the identical
//! manifest hash.

use std::fs;
use std::process::Command;

use ambient_engine::ast::Module;
use ambient_engine::build::{BuildOptions, ParseFailure, build_package};
use ambient_engine::disk_store::{BuildManifest, DiskStore};
use tempfile::TempDir;

fn ambient_bin() -> &'static str {
    env!("CARGO_BIN_EXE_ambient")
}

fn parse_source(source: &str) -> Result<Module, ParseFailure> {
    ambient_parser::parse(source).map_err(|e| ParseFailure {
        message: e.kind.to_string(),
        span: (e.span.start, e.span.end),
        context: e.context,
    })
}

/// Scaffold a one-module package that prints nothing and returns a number.
fn package() -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    fs::write(
        dir.path().join("ambient.toml"),
        "[package]\nname = \"snap_pkg\"\nversion = \"0.1.0\"\n",
    )
    .expect("manifest");
    let src = dir.path().join("src");
    fs::create_dir_all(&src).expect("src");
    fs::write(
        src.join("main.ab"),
        "fn gcd(a: Number, b: Number): Number { if b == 0 { a } else { gcd(b, a) } }\n\
         pub fn run(): Number { gcd(12, 0) }\n",
    )
    .expect("main");
    dir
}

/// Build the scaffolded package through the real pipeline (platform modules +
/// stub natives, exactly like `ambient run`) and fold it into a manifest.
fn manifest_of(dir: &TempDir) -> BuildManifest {
    let stubs = ambient_platform::stub_natives();
    let result = build_package(
        dir.path(),
        parse_source,
        &BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            progress: None,
            ..Default::default()
        },
    )
    .expect("build succeeds");
    BuildManifest::from_build(&result)
}

#[test]
fn identical_source_yields_identical_manifest_hash() {
    let a = package();
    let b = package();
    let ma = manifest_of(&a);
    let mb = manifest_of(&b);
    // Package names match, so the whole manifest — core, platform, and user
    // modules — is byte-identical.
    assert_eq!(ma.encode(), mb.encode());
    assert_eq!(ma.hash(), mb.hash());
}

#[test]
fn manifest_covers_core_platform_and_user_modules() {
    let dir = package();
    let manifest = manifest_of(&dir);
    let has = |m: &str| manifest.modules.iter().any(|x| x.module == m);
    assert!(has("workspace::snap_pkg::main"), "user module present");
    assert!(
        manifest
            .modules
            .iter()
            .any(|m| m.module.starts_with("core::")),
        "core modules present"
    );
    // The user module records its produced objects and its `run`/`gcd` names.
    let main = manifest
        .modules
        .iter()
        .find(|m| m.module == "workspace::snap_pkg::main")
        .expect("main module");
    assert!(!main.objects.is_empty(), "main produced objects");
    assert!(
        main.names.iter().any(|(n, _)| n.ends_with("::run")),
        "main binds run: {:?}",
        main.names
    );
}

/// A directory module (`<dir>/main.ab`) records its real on-disk path in the
/// manifest, not the `<dir>.ab` the canonical file↔module reconstruction would
/// produce. (It also has to *build* — reading through the reconstructed path
/// couldn't find `<dir>/main.ab` at all.)
#[test]
fn directory_module_records_its_main_ab_source_path() {
    let dir = TempDir::new().expect("temp dir");
    fs::write(
        dir.path().join("ambient.toml"),
        "[package]\nname = \"dirmod\"\nversion = \"0.1.0\"\n",
    )
    .expect("manifest");
    let src = dir.path().join("src");
    fs::create_dir_all(src.join("shapes")).expect("src/shapes");
    fs::write(
        src.join("main.ab"),
        "pub fn run(): Number { pkg::shapes::area() }\n",
    )
    .expect("main");
    fs::write(
        src.join("shapes").join("main.ab"),
        "pub fn area(): Number { 42 }\n",
    )
    .expect("shapes/main.ab");

    let manifest = manifest_of(&dir);
    let shapes = manifest
        .modules
        .iter()
        .find(|m| m.module == "workspace::dirmod::shapes")
        .expect("shapes directory module present");
    assert_eq!(
        shapes.source_path, "shapes/main.ab",
        "directory module records its real main.ab path, not a reconstructed shapes.ab"
    );
}

#[test]
fn run_records_a_snapshot_the_store_can_load() {
    let dir = package();

    // `ambient compile` builds the whole package and persists a snapshot
    // (`ambient run` is lazy and read-only — it writes no snapshot).
    let out = Command::new(ambient_bin())
        .arg("compile")
        .arg(dir.path())
        .output()
        .expect("compile");
    assert!(
        out.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The persisted snapshot loads and matches an in-process rebuild.
    let store = DiskStore::open(dir.path().join(".ambient").join("store")).expect("open store");
    let loaded = store
        .current_snapshot()
        .expect("load")
        .expect("snapshot present");
    assert_eq!(loaded.hash(), manifest_of(&dir).hash());

    // The store verifies clean, including its snapshot.
    let report = store.verify().expect("verify");
    assert!(report.is_clean(), "store not clean: {report:?}");
}

#[test]
fn store_snapshot_command_summarizes_the_build() {
    let dir = package();
    Command::new(ambient_bin())
        .arg("compile")
        .arg(dir.path())
        .output()
        .expect("compile");

    let out = Command::new(ambient_bin())
        .arg("store")
        .arg("--package")
        .arg(dir.path())
        .arg("snapshot")
        .output()
        .expect("store snapshot");
    assert!(
        out.status.success(),
        "store snapshot failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("package:"), "output: {stdout}");
    assert!(stdout.contains("snap_pkg"), "output: {stdout}");
    assert!(
        stdout.contains("workspace::snap_pkg::main"),
        "output: {stdout}"
    );
}

#[test]
fn store_snapshot_reports_absence_before_any_build() {
    let dir = package();
    // Open (and thus create) an empty store without ever building.
    DiskStore::open(dir.path().join(".ambient").join("store")).expect("open store");

    let out = Command::new(ambient_bin())
        .arg("store")
        .arg("--package")
        .arg(dir.path())
        .arg("snapshot")
        .output()
        .expect("store snapshot");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("no snapshot"), "output: {stdout}");
}

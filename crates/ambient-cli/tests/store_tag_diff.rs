//! End-to-end coverage of `ambient store tag` and `ambient store diff`:
//! tagging the current snapshot, listing tags, and diffing two snapshots
//! (body edit ⇒ rebound + body-only module change; signature edit ⇒ retired +
//! interface change; identical ⇒ empty; `--json` is valid and stable).

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn ambient_bin() -> &'static str {
    env!("CARGO_BIN_EXE_ambient")
}

/// Scaffold a package with the given `main.ab` body.
fn package(main: &str) -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    fs::write(
        dir.path().join("ambient.toml"),
        "[package]\nname = \"td_pkg\"\nversion = \"0.1.0\"\n",
    )
    .expect("manifest");
    let src = dir.path().join("src");
    fs::create_dir_all(&src).expect("src");
    fs::write(src.join("main.ab"), main).expect("main");
    dir
}

fn write_main(dir: &Path, main: &str) {
    fs::write(dir.join("src").join("main.ab"), main).expect("main");
}

fn build(dir: &Path) {
    let out = Command::new(ambient_bin())
        .arg("build")
        .arg(dir)
        .output()
        .expect("run");
    assert!(out.status.success(), "run failed: {}", stderr(&out));
}

fn store(dir: &Path, args: &[&str]) -> Output {
    Command::new(ambient_bin())
        .arg("store")
        .arg("--package")
        .arg(dir)
        .args(args)
        .output()
        .expect("store")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

const BASE: &str = "pub fn f(a: Number): Number { a }\npub fn run(): Number { f(1) }\n";

#[test]
fn tag_roundtrip_and_list() {
    let dir = package(BASE);
    build(dir.path());

    let out = store(dir.path(), &["tag", "v1.0"]);
    assert!(out.status.success(), "tag failed: {}", stderr(&out));
    assert!(stdout(&out).contains("tagged v1.0"), "{}", stdout(&out));

    let out = store(dir.path(), &["tag"]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("v1.0"), "list: {}", stdout(&out));
}

#[test]
fn diff_body_edit_is_rebound_and_body_only() {
    let dir = package(BASE);
    build(dir.path());
    store(dir.path(), &["tag", "base"]);

    // Change `f`'s body but not its signature.
    write_main(
        dir.path(),
        "pub fn f(a: Number): Number { a + 1 }\npub fn run(): Number { f(1) }\n",
    );
    build(dir.path());

    let out = store(dir.path(), &["diff", "base", "current"]);
    assert!(out.status.success(), "diff failed: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("(body-only)"), "expected body-only: {text}");
    assert!(
        text.contains("::f  [rebound]"),
        "expected f rebound: {text}"
    );
}

#[test]
fn diff_signature_edit_is_retired_and_interface() {
    let dir = package(BASE);
    build(dir.path());
    store(dir.path(), &["tag", "base"]);

    write_main(
        dir.path(),
        "pub fn f(a: Number): String { \"x\" }\npub fn run(): Number { 0 }\n",
    );
    build(dir.path());

    let out = store(dir.path(), &["diff", "base", "current"]);
    assert!(out.status.success(), "diff failed: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("(interface)"),
        "expected interface change: {text}"
    );
    assert!(
        text.contains("::f  [retired]"),
        "expected f retired: {text}"
    );
}

#[test]
fn diff_identical_is_empty_and_json_is_valid() {
    let dir = package(BASE);
    build(dir.path());

    let out = store(dir.path(), &["diff", "current", "current"]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("identical"), "{}", stdout(&out));

    // JSON of an identical diff parses and has empty collections.
    let out = store(dir.path(), &["diff", "current", "current", "--json"]);
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid json");
    assert_eq!(value["modules"]["changed"], serde_json::json!([]));
    assert_eq!(value["bindings"]["rebound"], serde_json::json!([]));
}

#[test]
fn diff_without_two_refs_errors() {
    let dir = package(BASE);
    build(dir.path());
    let out = store(dir.path(), &["diff"]);
    assert!(!out.status.success(), "diff with no refs must fail");
    assert!(
        stderr(&out).contains("two refs"),
        "error should explain: {}",
        stderr(&out)
    );
}

#[test]
fn gc_keeps_a_tagged_snapshot_loadable() {
    let dir = package(BASE);
    build(dir.path());
    store(dir.path(), &["tag", "keep"]);

    // A new build supersedes the tagged one, then gc runs.
    write_main(dir.path(), "pub fn run(): Number { 42 }\n");
    build(dir.path());
    let out = store(dir.path(), &["gc"]);
    assert!(out.status.success(), "gc failed: {}", stderr(&out));

    // The tag still diffs against current — its manifest and objects survived.
    let out = store(dir.path(), &["diff", "keep", "current"]);
    assert!(
        out.status.success(),
        "diff after gc failed: {}",
        stderr(&out)
    );
    // And the store verifies clean (no dangling tag).
    let out = store(dir.path(), &["verify"]);
    assert!(out.status.success(), "verify failed: {}", stderr(&out));
    assert!(stdout(&out).contains("clean"), "{}", stdout(&out));
}

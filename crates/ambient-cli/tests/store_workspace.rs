//! `ambient store` against a multi-package workspace store: snapshot-reading
//! subcommands (snapshot, list, show, tag, diff) follow one package's
//! snapshot pointer — implied inside a member directory, `--package <NAME>`
//! from the root — while whole-store subcommands (stats, verify, gc) never
//! need a selection.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn ambient_bin() -> &'static str {
    env!("CARGO_BIN_EXE_ambient")
}

/// A two-member workspace whose members produce distinct snapshots: `app`
/// (declaring struct `Gadget`) consumes `lib`, so app's manifest covers both
/// packages while lib's covers only itself. Each member is built separately
/// so the two snapshot pointers name different manifests.
fn built_workspace() -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    fs::write(
        dir.path().join("ambient.toml"),
        "[workspace]\nmembers = [\"app\", \"lib\"]\n",
    )
    .expect("workspace manifest");
    for member in ["app", "lib"] {
        let root = dir.path().join(member);
        fs::create_dir_all(root.join("src")).expect("member src");
        fs::write(
            root.join("ambient.toml"),
            format!("[package]\nname = \"{member}\"\nversion = \"0.1.0\"\n"),
        )
        .expect("member manifest");
    }
    fs::write(
        dir.path().join("lib/src/main.ab"),
        "pub fn greet(): Number { 40 }\n",
    )
    .expect("lib main");
    fs::write(
        dir.path().join("app/src/main.ab"),
        "use ::lib::greet;\n\
         pub unique(44444444-4444-4444-4444-444444444444) struct Gadget { n: Number }\n\
         pub fn run(): Number { greet() + 2 }\n",
    )
    .expect("app main");

    // Member builds repoint only their own snapshot, so building lib first
    // and app second leaves two pointers naming *different* manifests.
    for member in ["lib", "app"] {
        let out = Command::new(ambient_bin())
            .arg("build")
            .arg(dir.path().join(member))
            .output()
            .expect("build");
        assert!(
            out.status.success(),
            "build {member} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    dir
}

/// Run `ambient store <path> <args…>`.
fn store(path: &Path, args: &[&str]) -> Output {
    Command::new(ambient_bin())
        .arg("store")
        .arg(path)
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

#[test]
fn snapshot_selects_the_named_package() {
    let ws = built_workspace();

    let app = store(ws.path(), &["snapshot", "--package", "app"]);
    assert!(app.status.success(), "app snapshot: {}", stderr(&app));
    let text = stdout(&app);
    assert!(text.contains("package:          app"), "app: {text}");
    assert!(text.contains("workspace::app"), "app modules: {text}");
    // The note names the other package's snapshot so nothing looks lost.
    assert!(text.contains("others: lib"), "note should name lib: {text}");

    let lib = store(ws.path(), &["snapshot", "--package", "lib"]);
    assert!(lib.status.success(), "lib snapshot: {}", stderr(&lib));
    let lib_text = stdout(&lib);
    assert!(
        lib_text.contains("package:          lib"),
        "lib: {lib_text}"
    );

    // The two pointers name different manifests (separate member builds).
    let hash_line = |s: &str| {
        s.lines()
            .find(|l| l.starts_with("snapshot:"))
            .expect("snapshot line")
            .to_string()
    };
    assert_ne!(
        hash_line(&text),
        hash_line(&lib_text),
        "app and lib must show different manifests"
    );
}

#[test]
fn member_directory_implies_its_own_package() {
    let ws = built_workspace();
    let out = store(&ws.path().join("lib"), &["snapshot"]);
    assert!(out.status.success(), "implied member: {}", stderr(&out));
    assert!(
        stdout(&out).contains("package:          lib"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn pointer_reading_subcommands_require_a_choice_at_the_root() {
    let ws = built_workspace();
    for args in [&["snapshot"][..], &["list"][..], &["tag", "v1"][..]] {
        let out = store(ws.path(), args);
        assert!(!out.status.success(), "{args:?} must not guess a package");
        let err = stderr(&out);
        assert!(
            err.contains("--package") && err.contains("app") && err.contains("lib"),
            "{args:?} should list the members: {err}"
        );
    }
}

#[test]
fn whole_store_subcommands_need_no_package_at_the_root() {
    let ws = built_workspace();
    for args in [&["stats"][..], &["verify"][..], &["gc"][..]] {
        let out = store(ws.path(), args);
        assert!(
            out.status.success(),
            "{args:?} failed at the root: {}",
            stderr(&out)
        );
    }
}

#[test]
fn list_and_show_read_the_selected_snapshot() {
    let ws = built_workspace();

    // Every member's manifest records every member's *interface* (all
    // modules parse for name resolution), but only the built package's
    // items carry object hashes: app's build compiled `run`, lib's did not.
    let run_row = |package: &str| {
        let text = stdout(&store(
            ws.path(),
            &["list", "--kinds", "fn", "--package", package],
        ));
        text.lines()
            .find(|l| l.ends_with("workspace::app::run"))
            .unwrap_or_else(|| panic!("no run row for {package}: {text}"))
            .to_string()
    };
    assert!(
        !run_row("app").contains('—'),
        "app's own build hashes `run`: {}",
        run_row("app")
    );
    assert!(
        run_row("lib").contains('—'),
        "lib's build never compiled app: {}",
        run_row("lib")
    );

    // `show` resolves through the selected snapshot's structured index.
    let found = store(ws.path(), &["show", "Gadget", "--package", "app"]);
    assert!(found.status.success(), "show: {}", stderr(&found));
    assert!(stdout(&found).contains("(struct)"), "{}", stdout(&found));
}

#[test]
fn tag_and_diff_anchor_current_on_the_selected_package() {
    let ws = built_workspace();

    let tag = store(ws.path(), &["tag", "app-v1", "--package", "app"]);
    assert!(tag.status.success(), "tag app: {}", stderr(&tag));
    let tag = store(ws.path(), &["tag", "lib-v1", "--package", "lib"]);
    assert!(tag.status.success(), "tag lib: {}", stderr(&tag));

    // `current` == the selected package's own snapshot.
    let same = store(
        ws.path(),
        &["diff", "current", "current", "--package", "lib"],
    );
    assert!(same.status.success(), "diff: {}", stderr(&same));
    assert!(
        stdout(&same).contains("(snapshots are identical)"),
        "{}",
        stdout(&same)
    );

    // The two packages' snapshots are different manifests.
    let cross = store(
        ws.path(),
        &["diff", "app-v1", "current", "--package", "lib"],
    );
    assert!(cross.status.success(), "cross diff: {}", stderr(&cross));
    let text = stdout(&cross);
    assert!(
        text.contains("package: app -> lib"),
        "cross-package diff: {text}"
    );
}

#[test]
fn unknown_package_is_a_clear_error() {
    let ws = built_workspace();
    let out = store(ws.path(), &["snapshot", "--package", "ghost"]);
    assert!(!out.status.success(), "ghost must fail: {}", stdout(&out));
    let err = stderr(&out);
    assert!(
        err.contains("ghost") && err.contains("app") && err.contains("lib"),
        "the error should name the unknown package and the real ones: {err}"
    );
}

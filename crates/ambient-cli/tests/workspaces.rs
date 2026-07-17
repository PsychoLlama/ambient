//! Workspace end-to-end behavior through the real binary.
//!
//! A workspace groups first-party packages under one root `ambient.toml`
//! (`[workspace] members = [...]`): members reference each other with
//! `use ::<package>::…`, share the root's `.ambient` store, and build
//! lazily — `ambient build <member>` compiles only that package plus what
//! it needs. These tests pin the whole surface: discovery, cross-package
//! imports and visibility, the mount boundary, target selection, the
//! shared store's per-package snapshots, and cross-package cycles.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

fn ambient_bin() -> &'static str {
    env!("CARGO_BIN_EXE_ambient")
}

/// Write a workspace: a root manifest listing `members`, each member a
/// package from `(member, relative file, source)` triples.
fn workspace(members: &[&str], files: &[(&str, &str, &str)]) -> TempDir {
    let dir = TempDir::new().expect("create temp dir");
    let list = members
        .iter()
        .map(|m| format!("\"{m}\""))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(
        dir.path().join("ambient.toml"),
        format!("[workspace]\nmembers = [{list}]\n"),
    )
    .expect("workspace manifest");
    for member in members {
        let root = dir.path().join(member);
        fs::create_dir_all(root.join("src")).expect("member src");
        fs::write(
            root.join("ambient.toml"),
            format!("[package]\nname = \"{member}\"\nversion = \"0.1.0\"\n"),
        )
        .expect("member manifest");
    }
    for (member, rel, source) in files {
        let path = dir.path().join(member).join("src").join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(path, source).expect("module");
    }
    dir
}

/// Run `ambient <cmd> <path> [args…]`, returning success and combined
/// output with ANSI colors stripped.
fn invoke(cmd: &str, path: &Path, args: &[&str]) -> (bool, String) {
    let output = Command::new(ambient_bin())
        .arg(cmd)
        .arg(path)
        .args(args)
        .output()
        .expect("spawn ambient");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    (output.status.success(), strip_ansi(&combined))
}

/// Strip ANSI escape sequences (colors) from output.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for t in chars.by_ref() {
                    if t.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A two-member workspace: `app` consumes `lib` through the workspace
/// root — a root item, a submodule item, and a `pub use` re-export.
fn app_and_lib() -> TempDir {
    workspace(
        &["app", "lib"],
        &[
            (
                "lib",
                "main.ab",
                "pub use self::util::helper;\npub fn greet(): Number { 40 }\n",
            ),
            ("lib", "util.ab", "pub fn helper(): Number { 1 }\n"),
            (
                "app",
                "main.ab",
                "use ::lib::greet;\nuse ::lib::util;\nuse ::lib::helper;\n\
                 pub fn run(): Number { greet() + util::helper() + helper() }\n",
            ),
        ],
    )
}

#[test]
fn cross_package_imports_run() {
    let ws = app_and_lib();

    // From the member directory: the target package is implied.
    let (ok, out) = invoke("run", &ws.path().join("app"), &[]);
    assert!(ok, "run failed:\n{out}");
    assert_eq!(out.trim(), "42");

    // From the workspace root with an explicit target.
    let (ok, out) = invoke("run", ws.path(), &["--package", "app"]);
    assert!(ok, "run --package failed:\n{out}");
    assert_eq!(out.trim(), "42");
}

#[test]
fn check_agrees_across_the_workspace() {
    let ws = app_and_lib();

    // The whole workspace from the root…
    let (ok, out) = invoke("check", ws.path(), &[]);
    assert!(ok, "workspace check failed:\n{out}");

    // …and a member (which loads its siblings for `::lib` resolution).
    let (ok, out) = invoke("check", &ws.path().join("app"), &[]);
    assert!(ok, "member check failed:\n{out}");
}

#[test]
fn run_at_root_requires_a_package_choice() {
    let ws = app_and_lib();
    let (ok, out) = invoke("run", ws.path(), &[]);
    assert!(!ok, "run at a two-member root must not guess:\n{out}");
    assert!(
        out.contains("--package") && out.contains("app") && out.contains("lib"),
        "the error should list the members:\n{out}"
    );
}

#[test]
fn private_items_stay_private_across_packages() {
    let ws = workspace(
        &["app", "lib"],
        &[
            ("lib", "main.ab", "fn hidden(): Number { 1 }\n"),
            (
                "app",
                "main.ab",
                "use ::lib::hidden;\npub fn run(): Number { hidden() }\n",
            ),
        ],
    );
    for cmd in ["check", "run"] {
        let (ok, out) = invoke(cmd, &ws.path().join("app"), &[]);
        assert!(!ok, "{cmd} must reject a private import:\n{out}");
        assert!(
            out.contains("not public") || out.contains("private") || out.contains("hidden"),
            "{cmd} should explain the visibility failure:\n{out}"
        );
    }
}

#[test]
fn unknown_package_is_a_clear_error() {
    let ws = workspace(
        &["app"],
        &[(
            "app",
            "main.ab",
            "use ::ghost::thing;\npub fn run(): Number { thing() }\n",
        )],
    );
    let (ok, out) = invoke("check", &ws.path().join("app"), &[]);
    assert!(!ok, "import from a nonexistent package must fail:\n{out}");
    assert!(
        out.contains("ghost"),
        "the error should name the missing package:\n{out}"
    );
}

#[test]
fn super_cannot_escape_into_a_sibling_package() {
    // `super` from a member's top-level module would step above the
    // package root — the workspace namespace continues there, but the
    // language boundary must not.
    let ws = workspace(
        &["app", "lib"],
        &[
            ("lib", "main.ab", "pub fn greet(): Number { 40 }\n"),
            (
                "app",
                "deep.ab",
                "use super::super::lib;\npub fn f(): Number { lib::greet() }\n",
            ),
            ("app", "main.ab", "pub fn run(): Number { 1 }\n"),
        ],
    );
    let (ok, out) = invoke("check", &ws.path().join("app"), &[]);
    assert!(!ok, "super must not escape the package:\n{out}");
    assert!(
        out.contains("escapes package root"),
        "the error should name the escape:\n{out}"
    );
}

#[test]
fn cross_package_cycles_are_rejected() {
    let ws = workspace(
        &["a", "b"],
        &[
            (
                "a",
                "main.ab",
                "use ::b::bee;\npub fn ay(): Number { bee() }\npub fn run(): Number { ay() }\n",
            ),
            ("b", "main.ab", "use ::a::ay;\npub fn bee(): Number { 1 }\n"),
        ],
    );
    let (ok, out) = invoke("run", &ws.path().join("a"), &[]);
    assert!(!ok, "a cross-package cycle must fail:\n{out}");
    assert!(
        out.contains("import cycle: a -> b -> a"),
        "the cycle should name both packages:\n{out}"
    );
}

#[test]
fn duplicate_member_names_are_rejected() {
    let dir = TempDir::new().expect("create temp dir");
    fs::write(
        dir.path().join("ambient.toml"),
        "[workspace]\nmembers = [\"one\", \"two\"]\n",
    )
    .expect("workspace manifest");
    for member in ["one", "two"] {
        let root = dir.path().join(member);
        fs::create_dir_all(root.join("src")).expect("src");
        fs::write(
            root.join("ambient.toml"),
            "[package]\nname = \"same\"\nversion = \"0.1.0\"\n",
        )
        .expect("manifest");
        fs::write(root.join("src/main.ab"), "pub fn run(): Number { 1 }\n").expect("main");
    }
    let (ok, out) = invoke("run", &dir.path().join("one"), &[]);
    assert!(!ok, "duplicate names must fail:\n{out}");
    assert!(
        out.contains("duplicate package name"),
        "the error should name the collision:\n{out}"
    );
}

#[test]
fn member_builds_are_lazy() {
    // `other` has a type error; building `app` (which never touches it)
    // must succeed, while a workspace-root build must fail.
    let ws = workspace(
        &["app", "lib", "other"],
        &[
            ("lib", "main.ab", "pub fn greet(): Number { 42 }\n"),
            (
                "app",
                "main.ab",
                "use ::lib::greet;\npub fn run(): Number { greet() }\n",
            ),
            (
                "other",
                "main.ab",
                "pub fn broken(): Number { \"not a number\" }\n",
            ),
        ],
    );

    let (ok, out) = invoke("build", &ws.path().join("app"), &[]);
    assert!(ok, "member build must skip the unrelated member:\n{out}");

    let (ok, out) = invoke("build", ws.path(), &[]);
    assert!(!ok, "workspace-root build must reach `other`:\n{out}");

    let (ok, out) = invoke("build", ws.path(), &["--package", "app"]);
    assert!(ok, "--package narrows a root build:\n{out}");
}

#[test]
fn members_share_one_store_with_per_package_snapshots() {
    let ws = app_and_lib();

    let (ok, out) = invoke("build", ws.path(), &[]);
    assert!(ok, "workspace build failed:\n{out}");

    // One store at the workspace root; none inside members.
    let store = ws.path().join(".ambient/store");
    assert!(store.is_dir(), "shared store at the workspace root");
    assert!(!ws.path().join("app/.ambient").exists());
    assert!(!ws.path().join("lib/.ambient").exists());

    // A root build points every member's snapshot.
    let snapshots = store.join("snapshots");
    for member in ["app", "lib"] {
        assert!(
            snapshots.join(member).is_file(),
            "snapshot pointer for `{member}`"
        );
    }

    // A warm member run reuses the store (and reports the cached value).
    let (ok, out) = invoke("run", &ws.path().join("app"), &[]);
    assert!(ok, "warm run failed:\n{out}");
    assert_eq!(out.trim(), "42");
}

#[test]
fn pub_use_reexports_a_sibling_package() {
    let ws = workspace(
        &["app", "facade", "lib"],
        &[
            ("lib", "main.ab", "pub fn deep(): Number { 7 }\n"),
            ("facade", "main.ab", "pub use ::lib::deep;\n"),
            (
                "app",
                "main.ab",
                "use ::facade::deep;\npub fn run(): Number { deep() }\n",
            ),
        ],
    );
    let (ok, out) = invoke("run", &ws.path().join("app"), &[]);
    assert!(ok, "re-export across packages failed:\n{out}");
    assert_eq!(out.trim(), "7");
}

/// A library member needs no `main.ab`.
#[test]
fn library_members_need_no_entry_point() {
    let ws = workspace(
        &["app", "lib"],
        &[
            ("lib", "util.ab", "pub fn helper(): Number { 5 }\n"),
            (
                "app",
                "main.ab",
                "use ::lib::util::helper;\npub fn run(): Number { helper() }\n",
            ),
        ],
    );
    let (ok, out) = invoke("run", &ws.path().join("app"), &[]);
    assert!(ok, "library member without main.ab failed:\n{out}");
    assert_eq!(out.trim(), "5");

    let mut path = PathBuf::from(ws.path());
    path.push("lib");
    let (ok, out) = invoke("build", &path, &[]);
    assert!(ok, "building the library alone failed:\n{out}");
}

#[test]
fn inline_workspace_paths_run_without_imports() {
    // `::pkg::…` works directly in expression position — no `use` needed.
    // Covers a root item, a submodule item, and a type + variant in type
    // and pattern position.
    let ws = workspace(
        &["app", "lib"],
        &[
            (
                "lib",
                "main.ab",
                "pub unique(A1B2C3D4-0000-0000-0000-00000000AB01) enum Shape { Dot, Circle(Number) }\n\
                 pub fn greet(): Number { 40 }\n",
            ),
            ("lib", "util.ab", "pub fn helper(): Number { 1 }\n"),
            (
                "app",
                "main.ab",
                "fn area(shape: ::lib::Shape): Number {\n\
                 \x20   match shape {\n\
                 \x20       ::lib::Shape::Circle(r) => r,\n\
                 \x20       ::lib::Shape::Dot => 0,\n\
                 \x20   }\n\
                 }\n\
                 pub fn run(): Number {\n\
                 \x20   ::lib::greet() + ::lib::util::helper() + area(::lib::Shape::Circle(1))\n\
                 }\n",
            ),
        ],
    );

    let (ok, out) = invoke("run", &ws.path().join("app"), &[]);
    assert!(ok, "run failed:\n{out}");
    assert_eq!(out.trim(), "42");

    let (ok, out) = invoke("check", ws.path(), &[]);
    assert!(ok, "check failed:\n{out}");
}

#[test]
fn inline_workspace_paths_respect_visibility() {
    // A private item is no more reachable through an inline `::pkg` path
    // than through `use ::pkg` — same boundary, same error.
    let ws = workspace(
        &["app", "lib"],
        &[
            ("lib", "main.ab", "fn hidden(): Number { 1 }\n"),
            (
                "app",
                "main.ab",
                "pub fn run(): Number { ::lib::hidden() }\n",
            ),
        ],
    );

    let (ok, out) = invoke("check", &ws.path().join("app"), &[]);
    assert!(!ok, "private inline access should fail:\n{out}");
    assert!(out.contains("hidden"), "error should name the item:\n{out}");
}

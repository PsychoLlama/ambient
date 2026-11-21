//! Grants: what a dependent lets a dependency take, as a program runs
//! and as it is built. See "Grants" in `docs/spec/packages.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use ambient::cli::{Chosen, Command};
use support::scripted::Scripted;

type Files = BTreeMap<PathBuf, String>;

/// A library that prints, reads a file of its own as it is built, and
/// does neither.
const NOTES: &str = r#"
    use system.console;
    use ambient.build;
    use ambient.Error;

    fn read(path: string): string with build.read {
        match build.read!(path) {
            let error: Error => { return error.message; }
            let text: string => { return text; }
        }
    }

    pub fn kept(): string => comptime read("notes.txt");
    pub fn say(text: string): null with console.print { console.print!(text); }
    pub fn plain(): string => "plain";
"#;

/// A library over [`NOTES`], which it declares as `declared` says.
const OVER: &str = r"
    use notes;
    use system.console;

    pub fn say(text: string): null with console.print { notes.say(text); }
";

fn manifest(name: &str, rest: &str) -> String {
    format!("[package]\nname = \"{name}\"\nid = \"{name}\"\nversion = \"1.0.0\"\n{rest}")
}

/// A dependency on a vendored package, granted as `granted` says.
fn on(name: &str, granted: &str) -> String {
    format!("{name} = {{ id = \"{name}\", version = \"1.0.0\"{granted} }}\n")
}

/// A workspace of an app whose `main` is `body`, and two packages it
/// vendored: `notes`, and `over`, which calls it. The app declares
/// what `declared` says, and `over` grants `notes` what `passed` says.
fn workspace(body: &str, declared: &str, passed: &str) -> Files {
    let vendored = "[workspace]\nmembers = [\"app\", \"vendor/*\"]\n\
                    [workspace.vendored]\n\"vendor/notes\" = \"\"\n\"vendor/over\" = \"\"";
    let main = format!(
        "use notes;\nuse over;\nuse system.console;\n\
         fn main(): int with console.print {{ {body} return 0; }}"
    );
    let dependencies = |declared: &str| format!("[dependencies]\n{declared}");
    let files = [
        ("ambient.toml", vendored.to_owned()),
        ("app/ambient.toml", manifest("app", &dependencies(declared))),
        ("app/src/main.ab", main),
        (
            "vendor/notes/ambient.toml",
            manifest("notes", "files = [\"notes.txt\"]"),
        ),
        ("vendor/notes/src/lib.ab", NOTES.to_owned()),
        ("vendor/notes/notes.txt", "milk".to_owned()),
        (
            "vendor/over/ambient.toml",
            manifest("over", &dependencies(&on("notes", passed))),
        ),
        ("vendor/over/src/lib.ab", OVER.to_owned()),
    ];
    files
        .into_iter()
        .map(|(path, text)| (path.into(), text))
        .collect()
}

/// What running an app that declares `declared` prints and warns.
fn ran(files: &Files) -> (String, String) {
    let command = Command::Run {
        at: Chosen::default(),
        bin: None,
        args: Vec::new(),
    };
    let (_, world) = Scripted::within("app").execute(files, &command);
    (world.out, world.err)
}

/// What an app whose `main` is `body` prints and warns, with `notes`
/// granted as `granted` says and `over` nothing.
fn said(body: &str, granted: &str) -> (String, String) {
    let declared = on("notes", granted) + &on("over", "");
    ran(&workspace(body, &declared, ""))
}

const SAYS: &str = "notes.say(\"hello\");";
const PRINTS: &str = ", grants = [\"system.console.print\"]";

#[test]
fn a_dependency_takes_nothing_it_is_not_granted() {
    let (out, err) = said(SAYS, "");

    assert_eq!(out, "");
    let takes = "notes:.say: it takes `system:console.print`, which its package is not granted\n  \
                 where `app` declares `notes`, grant it: grants = [\"system.console.print\"]\n";
    assert!(err.ends_with(takes), "{err}");
}

#[test]
fn checking_holds_a_dependency_as_running_does() {
    let declared = on("notes", "") + &on("over", "");
    let check = Command::Check {
        at: Chosen::default(),
    };
    let files = workspace(SAYS, &declared, "");
    let (code, world) = Scripted::within("app").execute(&files, &check);

    assert_eq!(code, 1, "{}", world.err);
}

#[test]
fn a_dependency_takes_what_it_is_granted_by_ability_or_by_module() {
    let hello = ("hello\n".to_owned(), String::new());

    assert_eq!(said(SAYS, PRINTS), hello);
    assert_eq!(said(SAYS, ", grants = [\"system.console\"]"), hello);
}

#[test]
fn a_dependency_is_held_only_for_what_is_used_of_it() {
    let plain = "console.print!(notes.plain());";

    assert_eq!(said(plain, ""), ("plain\n".to_owned(), String::new()));
}

#[test]
fn a_comptime_call_of_a_dependency_takes_what_the_dependency_is_granted() {
    let kept = "console.print!(notes.kept());";
    let (out, err) = said(kept, "");

    assert_eq!(out, "");
    assert!(
        err.contains("`ambient:build.read`, which its package is not granted"),
        "{err}"
    );
    let reads = ", grants = [\"ambient.build.read\"]";
    assert_eq!(said(kept, reads), ("milk\n".to_owned(), String::new()));
}

#[test]
fn a_dependency_grants_what_it_depends_on_only_what_it_holds() {
    let through = |granted: &str| {
        let declared = on("notes", "") + &on("over", granted);
        ran(&workspace("over.say(\"hello\");", &declared, PRINTS))
    };

    assert_eq!(through(PRINTS), ("hello\n".to_owned(), String::new()));
    // What `over` grants is nothing it was granted itself.
    let (out, err) = through("");
    assert_eq!(out, "");
    assert!(err.contains("which its package is not granted"), "{err}");
}

#[test]
fn a_grant_names_an_ability_or_a_module() {
    let (_, err) = said(SAYS, ", grants = [\"system.console.shout\"]");

    let unknown = "error: app: the grant `system.console.shout` to `notes` names no ability and \
                   no module it can reach\n";
    assert_eq!(err, unknown);
}

#[test]
fn a_package_at_a_directory_is_granted_nothing_since_it_takes_anything() {
    let beside = "notes = { path = \"../vendor/notes\", grants = [] }\n";
    let (_, err) = ran(&workspace(SAYS, beside, ""));

    let refused = "error: app/ambient.toml: the dependency `notes` says what a package at a \
                   `path` is granted, which is everything\n";
    assert_eq!(err, refused);
}

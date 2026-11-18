//! Tools: a target built by a function of a package, which asks the
//! build what it needs and gives the files. See `spec/tools.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use ambient::cli::Command;
use ambient::store::MemoryStore;
use support::scripted::Scripted;

type Files = BTreeMap<PathBuf, String>;

/// A program for a page, and the tool its target is built by: the
/// module, and a line for each extern it imports with its host code.
const PAGE: &str = r#"
    import ambient.build;
    import ambient.build.{Compiled, Extern, Files};
    import ambient.tree.{Reference};
    import ambient.Error;

    ability say(text: string): null;
    extern handler said for say;
    target page built by bundle { say: said }

    fn main(): int with say {
        say!("hello");
        return 3;
    }

    fn hosted(native: Extern): string with build.host_code {
        let code = build.host_code!(native.package, "js", "main.js") as string else "nothing";
        return "${native.path}: ${code}";
    }

    fn bundle(entry: Reference, target: Reference): Files with build.compile, build.host_code {
        match build.compile!(entry, target) {
            let error: Error => {
                return [{ path: "error.txt", bytes: error.message.to_bytes() }];
            }
            let compiled: Compiled => {
                let lines = compiled.externs.map(fn(native: Extern): string => hosted(native));
                return [
                    { path: "app.wasm", bytes: compiled.module },
                    { path: "notes/externs.txt", bytes: lines.join_with(", ").to_bytes() },
                ];
            }
        }
    }
"#;

const GRANTED: &str = "grants = [\"ambient.build.compile\", \"ambient.build.host_code\"]";

/// A package whose one module is `source`, with host code for
/// JavaScript, and whose manifest ends with `grants`.
fn package(source: &str, grants: &str) -> Files {
    let manifest =
        format!("name = \"app\"\nid = \"app\"\nversion = \"0.1.0\"\nfiles = [\"js\"]\n{grants}");
    let files = [
        ("app/ambient.toml", manifest),
        ("app/src/main.ab", source.to_owned()),
        ("app/js/main.js", "export const said = print;".to_owned()),
    ];
    let file = |(path, text): (&str, String)| (PathBuf::from(path), text);
    files.into_iter().map(file).collect()
}

/// Builds `main` of the package under its target `page` in `world`,
/// and gives the world it left.
fn build(files: &Files, world: Scripted) -> Scripted {
    let command = Command::Build {
        path: PathBuf::from("app"),
        entry: "main.main".to_owned(),
        output: Some(PathBuf::from("out")),
        target: Some("main.page".to_owned()),
    };
    world.execute(files, &command).1
}

fn text(world: &Scripted, path: &str) -> String {
    String::from_utf8_lossy(&world.files[path]).into_owned()
}

#[test]
fn a_target_built_by_a_tool_gives_the_files_the_tool_does() {
    use ambient::backend::native::Natives;
    use ambient::backend::wasm::Program;
    use ambient::value::{Value, decode};

    let built = build(&package(PAGE, GRANTED), Scripted::default());

    assert_eq!(built.err, "");
    assert!(
        built.out.starts_with("built out: 2 files, "),
        "{}",
        built.out
    );
    assert_eq!(
        text(&built, "out/notes/externs.txt"),
        "main.said: export const said = print;"
    );
    let natives = Natives::new().with("app", "main.said", |_| Ok(Value::Null));
    let mut program = Program::load(&built.files["out/app.wasm"], &natives).unwrap();
    let gave = program.call(&[]).unwrap();
    assert_eq!(decode(gave, program.guest()), Value::Int(3));
}

#[test]
fn a_build_takes_away_what_the_one_before_wrote_and_it_does_not_give() {
    let renamed = PAGE.replace("notes/externs.txt", "externs.txt");
    let kept = ("out/mine.txt".to_owned(), b"kept".to_vec());

    let first = build(&package(PAGE, GRANTED), Scripted::default());
    let world = Scripted {
        files: first.files.into_iter().chain([kept]).collect(),
        ..Scripted::default()
    };
    let second = build(&package(&renamed, GRANTED), world);

    let written: Vec<&str> = second.files.keys().map(String::as_str).collect();
    assert_eq!(
        written,
        [
            "out/.ambient-build",
            "out/app.wasm",
            "out/externs.txt",
            "out/mine.txt"
        ]
    );
}

#[test]
fn a_build_follows_the_host_code_its_tool_read() {
    let store = MemoryStore::new();
    let command = Command::Build {
        path: PathBuf::from("app"),
        entry: "main.main".to_owned(),
        output: Some(PathBuf::from("out")),
        target: Some("main.page".to_owned()),
    };
    let externs = |code: &str| {
        let mut files = package(PAGE, GRANTED);
        files.insert(PathBuf::from("app/js/main.js"), code.to_owned());
        let (_, built) = Scripted::default().execute_over(&files, &store, &command);
        text(&built, "out/notes/externs.txt")
    };

    assert_eq!(externs("one"), "main.said: one");
    assert_eq!(externs("two"), "main.said: two");
}

#[test]
fn a_tool_takes_only_what_its_package_is_granted() {
    let built = build(
        &package(PAGE, "grants = [\"ambient.build.compile\"]"),
        Scripted::default(),
    );

    assert!(
        built.err.contains(
            "app:main.bundle: the tool takes `ambient:build.host_code`, which its package is not \
             granted"
        ),
        "{}",
        built.err
    );
    assert!(built.files.is_empty());
}

#[test]
fn what_a_target_is_built_by_takes_an_entry_and_a_target_and_gives_files() {
    let source = PAGE.replace("built by bundle", "built by hosted");

    let built = build(&package(&source, GRANTED), Scripted::default());

    assert!(
        built
            .err
            .contains("app:main.hosted: what builds a program takes"),
        "{}",
        built.err
    );
}

#[test]
fn a_tool_that_cannot_compile_its_program_is_told_why() {
    let source = PAGE.replace(
        "target page built by bundle { say: said }",
        "target page built by bundle {}",
    );

    let built = build(&package(&source, GRANTED), Scripted::default());

    assert!(
        text(&built, "out/error.txt").contains("say"),
        "{}",
        built.err
    );
}

#[test]
fn a_file_a_build_gives_is_under_its_directory() {
    let source = PAGE.replace("notes/externs.txt", "../externs.txt");

    let built = build(&package(&source, GRANTED), Scripted::default());

    assert!(
        built
            .err
            .contains("`../externs.txt`, which is not a file under its directory"),
        "{}",
        built.err
    );
    assert!(built.files.is_empty());
}

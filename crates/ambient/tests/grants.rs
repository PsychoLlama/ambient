//! Build grants: what a package's manifest lets its comptime calls
//! take, and the files they read. See "Authority" in
//! `spec/comptime.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use ambient::cli::Command;
use ambient::store::MemoryStore;
use support::scripted::Scripted;

type Files = BTreeMap<PathBuf, String>;

/// A program that prints what a comptime call read of `path`.
const READS: &str = r#"
    import system.console;
    import ambient.build;
    import ambient.Error;

    fn read(path: string): string with build.read {
        match build.read!(path) {
            let error: Error => { return error.message; }
            let text: string => { return text; }
        }
    }

    fn main(): int with console.print {
        console.print!(comptime read("PATH"));
        return 0;
    }
"#;

/// A package whose manifest ends with `grants`, with a file of notes
/// that says `notes`, and a program that reads `path` as it is built.
fn package(grants: &str, notes: &str, path: &str) -> Files {
    let file = |(path, text): (&str, String)| (PathBuf::from(path), text);
    let files = [
        (
            "app/ambient.toml",
            format!("name = \"app\"\nid = \"app\"\nversion = \"0.1.0\"\n{LISTED}\n{grants}"),
        ),
        ("app/src/main.ab", READS.replace("PATH", path)),
        ("app/notes.txt", notes.to_owned()),
        ("app/kept/one.txt", String::new()),
        ("app/kept/two.txt", String::new()),
        ("app/draft.txt", "unfinished".to_owned()),
        ("secret.txt", "hidden".to_owned()),
    ];
    files.into_iter().map(file).collect()
}

/// The files beside its sources that the package says are its own.
const LISTED: &str = "files = [\"notes.txt\", \"kept\"]";

const GRANTED: &str = "grants = [\"ambient.build.read\"]";

/// What running the package over `store` prints and warns.
fn run(files: &Files, store: &MemoryStore) -> (String, String) {
    let command = Command::Run {
        path: PathBuf::from("app"),
        entry: "main.main".to_owned(),
        args: Vec::new(),
    };
    let (_, world) = Scripted::default().execute_over(files, store, &command);
    (world.out, world.err)
}

#[test]
fn a_granted_call_reads_a_file_of_its_package() {
    let files = package(GRANTED, "milk", "notes.txt");

    assert_eq!(
        run(&files, &MemoryStore::new()),
        ("milk\n".to_owned(), String::new())
    );
}

#[test]
fn a_call_takes_nothing_its_package_was_not_granted() {
    let (out, err) = run(&package("", "milk", "notes.txt"), &MemoryStore::new());

    assert_eq!(out, "");
    assert!(err.contains("ambient:build.read"), "{err}");
}

#[test]
fn a_build_follows_the_files_it_read() {
    let store = MemoryStore::new();
    let said = |notes: &str| run(&package(GRANTED, notes, "notes.txt"), &store).0;

    assert_eq!(said("milk"), "milk\n");
    assert_eq!(said("eggs"), "eggs\n");
}

#[test]
fn a_build_reads_nothing_outside_its_package() {
    let said = |path: &str| run(&package(GRANTED, "milk", path), &MemoryStore::new()).0;

    let refused = |path: &str| format!("{path} is not a file of the package\n");
    assert_eq!(said("../secret.txt"), refused("../secret.txt"));
    assert_eq!(said("/secret.txt"), refused("/secret.txt"));
}

#[test]
fn a_build_reads_only_the_files_its_package_lists() {
    let said = |path: &str| run(&package(GRANTED, "milk", path), &MemoryStore::new()).0;

    assert_eq!(
        said("draft.txt"),
        "draft.txt is not a file of the package\n"
    );
    assert_eq!(said("./notes.txt"), "milk\n");
    assert!(said("ambient.toml").starts_with("name = \"app\""));
}

#[test]
fn a_granted_call_lists_a_directory_of_its_package() {
    let lists = READS
        .replace("build.read!(path)", "build.list!(path)")
        .replace("build.read", "build.list")
        .replace(
            "let text: string => { return text; }",
            "let names: List<string> => { return names.join_with(\" \"); }",
        );
    let mut files = package("grants = [\"ambient.build.list\"]", "milk", "kept");
    files.insert(
        PathBuf::from("app/src/main.ab"),
        lists.replace("PATH", "kept"),
    );

    assert_eq!(run(&files, &MemoryStore::new()).0, "one.txt two.txt\n");
}

#[test]
fn a_grant_names_an_ability() {
    let files = package("grants = [\"ambient.build.peek\"]", "milk", "notes.txt");
    let (out, err) = run(&files, &MemoryStore::new());

    assert_eq!(out, "");
    assert_eq!(
        err,
        "error: app: the grant `ambient.build.peek` names no ability it can reach\n"
    );
}

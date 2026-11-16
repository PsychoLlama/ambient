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
    import system.fs;
    import ambient.Error;

    fn read(path: string): string with fs.read {
        match fs.read!(path) {
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
            format!("name = \"app\"\nid = \"app\"\n{grants}"),
        ),
        ("app/src/main.ab", READS.replace("PATH", path)),
        ("app/notes.txt", notes.to_owned()),
        ("secret.txt", "hidden".to_owned()),
    ];
    files.into_iter().map(file).collect()
}

const GRANTED: &str = "grants = [\"system.fs.read\"]";

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
    assert!(err.contains("system:fs.read"), "{err}");
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

    assert_eq!(
        said("../secret.txt"),
        "../secret.txt is not in the package\n"
    );
    assert_eq!(said("/secret.txt"), "/secret.txt is not in the package\n");
}

#[test]
fn a_granted_call_lists_a_directory_of_its_package() {
    let lists = READS
        .replace("fs.read!(path)", "fs.list!(path)")
        .replace("fs.read", "fs.list")
        .replace(
            "let text: string => { return text; }",
            "let names: List<string> => { return names.join_with(\" \"); }",
        );
    let mut files = package("grants = [\"system.fs.list\"]", "milk", ".");
    files.insert(PathBuf::from("app/src/main.ab"), lists.replace("PATH", "."));

    assert_eq!(
        run(&files, &MemoryStore::new()).0,
        "ambient.toml notes.txt src\n"
    );
}

#[test]
fn a_grant_names_an_ability() {
    let files = package("grants = [\"system.fs.peek\"]", "milk", "notes.txt");
    let (out, err) = run(&files, &MemoryStore::new());

    assert_eq!(out, "");
    assert_eq!(
        err,
        "error: app: the grant `system.fs.peek` names no ability it can reach\n"
    );
}

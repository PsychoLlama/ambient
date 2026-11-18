//! A package's snapshot: a hash of the files that are it, and of
//! nothing else. See "Identity" in `spec/packages.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use ambient::cli::Command;
use support::scripted::Scripted;

type Files = BTreeMap<PathBuf, String>;

/// A package at `at` that lists `notes.txt` and the directory `kept`,
/// among files it does not list.
fn package(at: &str) -> Files {
    let files = [
        (
            "ambient.toml",
            "name = \"app\"\nid = \"app\"\nversion = \"1.0.0\"\nfiles = [\"notes.txt\", \"kept\"]",
        ),
        ("src/main.ab", "pub fn main(): int => 1;"),
        ("src/data/table.csv", "1,2"),
        ("notes.txt", "milk"),
        ("kept/deep/one.txt", "one"),
        ("draft.txt", "unfinished"),
    ];
    let file = |(path, text): (&str, &str)| (PathBuf::from(at).join(path), text.to_owned());
    files.into_iter().map(file).collect()
}

/// The snapshot of the package at `at`, as the command line says it.
fn snapshot(files: &Files, at: &str) -> String {
    let path = PathBuf::from(at);
    let (code, world) = Scripted::default().execute(files, &Command::Snapshot { path });
    assert_eq!((code, world.err.as_str()), (0, ""));
    world.out
}

/// The snapshot of the package once the file at `path` says `text`.
fn changed(path: &str, text: &str) -> String {
    let mut files = package("app");
    files.insert(PathBuf::from("app").join(path), text.to_owned());
    snapshot(&files, "app")
}

#[test]
fn a_snapshot_is_a_hash_written_out() {
    let said = snapshot(&package("app"), "app");

    assert!(
        ambient::hash::Hash::parse(said.trim_end()).is_some(),
        "{said}"
    );
}

#[test]
fn a_snapshot_is_the_same_wherever_the_package_is() {
    let here = snapshot(&package("app"), "app");

    assert_eq!(
        snapshot(&package("vendored/copy/of"), "vendored/copy/of"),
        here
    );
}

#[test]
fn a_snapshot_covers_every_file_of_the_package() {
    let unchanged = snapshot(&package("app"), "app");

    for file in [
        "ambient.toml",
        "src/main.ab",
        "src/data/table.csv",
        "notes.txt",
        "kept/deep/one.txt",
    ] {
        let text = package("app")[&PathBuf::from("app").join(file)].clone();
        assert_ne!(changed(file, &format!("{text}\n")), unchanged, "{file}");
    }
    assert_ne!(changed("kept/two.txt", ""), unchanged);
    assert_ne!(changed("src/more.ab", ""), unchanged);
}

#[test]
fn a_snapshot_covers_nothing_the_package_does_not_list() {
    let unchanged = snapshot(&package("app"), "app");

    assert_eq!(changed("draft.txt", "finished"), unchanged);
    assert_eq!(changed("target/built", "bytes"), unchanged);
}

#[test]
fn a_file_is_told_from_its_name() {
    // What one file held, another holds: the files are not the same.
    let mut files = package("app");
    let notes = files.remove(&PathBuf::from("app/notes.txt")).unwrap();
    files.insert(PathBuf::from("app/kept/notes.txt"), notes);
    files.insert(PathBuf::from("app/notes.txt"), String::new());

    assert_ne!(snapshot(&files, "app"), snapshot(&package("app"), "app"));
}

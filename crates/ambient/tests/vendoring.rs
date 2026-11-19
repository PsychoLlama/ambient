//! Copying a dependency into a workspace as a member. See "Vendoring"
//! in `spec/releases.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::path::PathBuf;

use ambient::cli::Command;
use support::packages::{Files, Package, files, package, ran, ran_at, workspace, written};
use support::scripted::Scripted;

const SEVEN: &str = "pub fn seven(): int => 7;";

/// An `app` whose `main` gives what `seven` of the package it calls
/// `numbers` gives, which is at `../far/numbers`.
fn app() -> Package {
    let app = package("app", "app").on("numbers", "n", "path = \"../far/numbers\"");
    app.library(
        "use numbers.seven;
        pub fn main(): int => seven();",
    )
}

fn numbers() -> Package {
    Package::versioned("far/numbers", "n", "1.2.0").library(SEVEN)
}

fn command(name: Option<&str>) -> Command {
    Command::Vendor {
        name: name.map(str::to_owned),
        path: PathBuf::from("app"),
    }
}

/// `files` once `app` has vendored what it calls `numbers`.
fn vendored(files: Files) -> Result<Files, String> {
    written(files, &command(Some("numbers")))
}

/// What the command line says of the vendored copies of `files`.
fn kept(files: &Files) -> String {
    Scripted::default().execute(files, &command(None)).1.out
}

/// The snapshot of the package at `at`.
fn snapshot(files: &Files, at: &str) -> String {
    let path = PathBuf::from(at);
    let said = Scripted::default().execute(files, &Command::Snapshot { path });
    said.1.out
}

fn file(files: &Files, path: &str) -> String {
    files[&PathBuf::from(path)].clone()
}

#[test]
fn a_vendored_package_is_built_from_its_copy() {
    let mut files = vendored(files(None, [app(), numbers()])).unwrap();
    assert_eq!(ran(&files), Ok(7));

    // The original is no longer read.
    files.retain(|path, _| !path.starts_with("far"));
    assert_eq!(ran(&files), Ok(7));
    let copy = "app/vendor/numbers/src/lib.ab";
    files.insert(copy.into(), "pub fn seven(): int => 8;".to_owned());
    assert_eq!(ran(&files), Ok(8));
}

#[test]
fn a_copy_is_the_snapshot_it_was_copied_from_until_it_is_changed() {
    let before = files(None, [app(), numbers()]);
    let mut files = vendored(before.clone()).unwrap();

    let original = snapshot(&before, "far/numbers");
    assert_eq!(snapshot(&files, "app/vendor/numbers"), original);
    let record = format!("\"vendor/numbers\" = \"{}\"", original.trim_end());
    assert!(file(&files, "app/ambient.toml").contains(&record));
    assert_eq!(kept(&files), "app/vendor/numbers: as copied\n");

    files.insert("app/vendor/numbers/src/more.ab".into(), String::new());
    assert_eq!(kept(&files), "app/vendor/numbers: changed\n");
}

#[test]
fn a_dependent_no_longer_names_the_directory_of_the_original() {
    let files = vendored(files(None, [app(), numbers()])).unwrap();

    let manifest = file(&files, "app/ambient.toml");
    assert!(
        manifest.contains("numbers = { id = \"n\", version = \"1.2.0\" }"),
        "{manifest}"
    );
    assert!(
        manifest.contains("members = [\"vendor/numbers\"]"),
        "{manifest}"
    );
}

#[test]
fn a_copy_goes_to_the_workspace_and_every_member_that_was_opened_depends_on_it() {
    // `twice` is beside `app` in a workspace, and names the original
    // too.
    let far = "path = \"../../far/numbers\"";
    let twice = package("work/twice", "t")
        .on("n", "n", far)
        .library("use n.seven; pub fn fourteen(): int => seven() + seven();");
    let app = package("work/app", "app")
        .on("numbers", "n", far)
        .on("twice", "t", "version = \"1.0.0\"")
        .library(
            "use numbers.seven; use twice.fourteen;
            pub fn main(): int => seven() + fourteen();",
        );
    let mut files = files(None, [app, twice, numbers()]);
    files.insert("work/ambient.toml".into(), workspace(&["app", "twice"]));
    let vendor = Command::Vendor {
        name: Some("numbers".to_owned()),
        path: PathBuf::from("work/app"),
    };
    let mut files = written(files, &vendor).unwrap();

    files.retain(|path, _| !path.starts_with("far"));
    assert_eq!(ran_at(&files, "work/app"), Ok(21));
    assert_eq!(
        file(&files, "work/ambient.toml"),
        format!(
            "members = [\"app\", \"twice\", \"vendor/numbers\"]\n\n\
             [vendored]\n\"vendor/numbers\" = \"{}\"\n",
            snapshot(&files, "work/vendor/numbers").trim_end()
        )
    );
}

#[test]
fn a_package_is_vendored_once_and_only_from_outside_the_workspace() {
    let refused = |files: Files| vendored(files).unwrap_err();
    let inside = app().on("near", "m", "path = \"../near\"");
    let near = package("near", "m").library(SEVEN);
    let listed = files(Some(&["app", "near"]), [inside, near, numbers()]);
    let near = Command::Vendor {
        name: Some("near".to_owned()),
        path: PathBuf::from("app"),
    };

    assert_eq!(
        written(listed, &near).unwrap_err(),
        "error: app/ambient.toml: `near` is in the workspace already\n"
    );
    assert_eq!(
        refused(vendored(files(None, [app(), numbers()])).unwrap()),
        "error: app/ambient.toml: `numbers` is in the workspace already\n"
    );
}

#[test]
fn a_package_that_names_a_directory_is_not_copied_away_from_it() {
    let numbers = numbers().on("other", "o", "path = \"../other\"");
    let other = package("far/other", "o");

    assert_eq!(
        vendored(files(None, [app(), numbers, other])).unwrap_err(),
        "error: app/ambient.toml: `numbers` depends on a package by its directory, \
         so a copy would not\n"
    );
}

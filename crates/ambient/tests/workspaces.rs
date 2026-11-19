//! Packages that depend on each other, by directory and as members of
//! a workspace. See "Workspaces" in `docs/spec/packages.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::path::PathBuf;

use ambient::cli::Command;
use support::packages::{Files, Package, files, package, ran, written};
use support::scripted::Scripted;

/// An `app` whose `main` gives what `seven` of the package it calls
/// `numbers` gives.
fn app() -> Package {
    package("app", "app").library(
        "use numbers.seven;
        pub fn main(): int => seven();",
    )
}

const SEVEN: &str = "pub fn seven(): int => 7;";

#[test]
fn a_member_stands_in_for_a_dependency_on_its_package() {
    let files = files(
        Some(&["app", "kept/numbers"]),
        [
            app().on("numbers", "n", "version = \"1.0.0\""),
            package("kept/numbers", "n").library(SEVEN),
        ],
    );

    assert_eq!(ran(&files), Ok(7));
}

#[test]
fn a_package_at_a_directory_a_member_depends_on_is_a_member_too() {
    // `numbers` is listed nowhere, and stands in for `twice`'s
    // dependency all the same.
    let twice = package("twice", "t")
        .on("n", "n", "version = \"1.0.0\"")
        .library("use n.seven; pub fn fourteen(): int => seven() + seven();");
    let app = app()
        .on("numbers", "n", "path = \"../numbers\"")
        .on("twice", "t", "path = \"../twice\"")
        .library(
            "use numbers.seven; use twice.fourteen;
            pub fn main(): int => seven() + fourteen();",
        );
    let files = files(None, [app, twice, package("numbers", "n").library(SEVEN)]);

    assert_eq!(ran(&files), Ok(21));
}

#[test]
fn packages_may_depend_on_each_other() {
    let even = package("even", "e")
        .on("odd", "o", "path = \"../odd\"")
        .library(
            "use odd.is_odd;
            pub fn is_even(n: int): bool {
                if n == 0 { return true; }
                return is_odd(n - 1);
            }",
        );
    let odd = package("odd", "o")
        .on("even", "e", "path = \"../even\"")
        .library(
            "use even.is_even;
            pub fn is_odd(n: int): bool {
                if n == 0 { return false; }
                return is_even(n - 1);
            }",
        );
    let app = package("app", "app")
        .on("even", "e", "path = \"../even\"")
        .library(
            "use even.is_even;
            pub fn main(): int {
                if is_even(10) { return 1; }
                return 0;
            }",
        );

    assert_eq!(ran(&files(None, [app, even, odd])), Ok(1));
}

/// What `app` gives when it depends on the package `n` at least at
/// each of two versions, and a member is at each.
fn side_by_side(old: &str, new: &str) -> Result<i32, String> {
    let app = package("app", "app")
        .on("numbers", "n", &format!("version = \"{old}\""))
        .on("numbers_next", "n", &format!("version = \"{new}\""))
        .library(
            "use numbers.seven; use numbers_next.seventy;
            pub fn main(): int => seven() + seventy();",
        );
    let old = Package::versioned("old", "n", old).library(SEVEN);
    let new = Package::versioned("new", "n", new).library("pub fn seventy(): int => 70;");
    ran(&files(Some(&["app", "old", "new"]), [app, old, new]))
}

#[test]
fn two_lines_of_a_package_are_two_members_under_two_names() {
    assert_eq!(side_by_side("1.4.0", "2.0.0"), Ok(77));
    // Under major zero a minor is a line, and under that a patch.
    assert_eq!(side_by_side("0.1.4", "0.2.0"), Ok(77));
    assert_eq!(side_by_side("0.0.1", "0.0.2"), Ok(77));
}

/// What `app` gives when it hands a branded value that the member
/// `old` made to the member `new`, each a package with the ID given.
fn exchanged(old: &str, new: &str) -> Result<i32, String> {
    const UNITS: &str = "pub type(nominal) Meters;
        pub fn meters(n: int): int & Meters => Meters(n);
        pub fn length(m: int & Meters): int => m;";
    let app = package("app", "app")
        .on("units", old, "version = \"1.0.0\"")
        .on("units_next", new, "version = \"2.0.0\"")
        .library(
            "use units.meters; use units_next.length;
            pub fn main(): int => length(meters(7));",
        );
    let old = Package::versioned("old", old, "1.0.0").library(UNITS);
    let new = Package::versioned("new", new, "2.0.0").library(UNITS);
    ran(&files(Some(&["app", "old", "new"]), [app, old, new]))
}

#[test]
fn two_instances_of_a_package_share_what_is_the_same_in_both() {
    assert_eq!(exchanged("units", "units"), Ok(7));
    // The same text in another package is another brand.
    assert!(exchanged("units", "others").is_err());
}

#[test]
fn two_members_in_one_line_of_a_package_are_refused() {
    let said = side_by_side("0.2.0", "0.2.5").unwrap_err();
    assert_eq!(
        said,
        "error: two members are package `n` in the version line `0.2`: old and new\n"
    );
}

#[test]
fn a_dependency_no_member_stands_in_for_is_refused() {
    let numbers = Package::versioned("numbers", "n", "2.0.0").library(SEVEN);
    let files = files(
        Some(&["app", "numbers"]),
        [app().on("numbers", "n", "version = \"1.0.0\""), numbers],
    );

    let said = "error: app: the dependency `numbers` is on package `n` in the version line `1`, \
                which no member of the workspace is\n";
    assert_eq!(ran(&files), Err(said.to_owned()));
}

#[test]
fn a_dependency_at_a_directory_is_on_the_package_it_says() {
    let run = |declared: &str| {
        let numbers = Package::versioned("numbers", "n", "1.2.0").library(SEVEN);
        let app = app().on("numbers", declared, "path = \"../numbers\"");
        ran(&files(None, [app, numbers]))
    };

    assert_eq!(run("n"), Ok(7));
    let said = "error: app: the dependency `numbers` says package `m`, \
                and the package at numbers is `n`\n";
    assert_eq!(run("m"), Err(said.to_owned()));
}

#[test]
fn a_workspace_that_does_not_list_a_package_is_not_its_workspace() {
    // `numbers` is a member of the workspace above, which `app` is
    // not in: nothing stands in.
    let files = files(
        Some(&["numbers"]),
        [
            app().on("numbers", "n", "version = \"1.0.0\""),
            package("numbers", "n").library(SEVEN),
        ],
    );

    assert!(ran(&files).is_err());
}

/// A workspace of two packages, each with the library `one` and
/// `other` give it, whose own directory is no package.
fn pair(one: &'static str, other: &'static str) -> Files {
    let packages = [
        package("one", "one").library(one),
        package("other", "other").library(other),
    ];
    files(Some(&["one", "other"]), packages)
}

/// What `command` comes to at the workspace's directory: the code it
/// exits with and all it says.
fn at_the_top(files: &Files, command: &Command) -> (i32, String) {
    let (code, world) = Scripted::default().execute(files, command);
    (code, world.out + &world.err)
}

#[test]
fn checking_a_workspace_checks_every_member() {
    let check = Command::Check { path: ".".into() };
    let wrong = "pub fn wrong(): int => \"no\";";

    assert_eq!(at_the_top(&pair(SEVEN, SEVEN), &check), (0, String::new()));
    let (code, said) = at_the_top(&pair(wrong, wrong), &check);
    assert_ne!(code, 0);
    assert!(
        said.contains("one/src/lib.ab:1:") && said.contains("other/src/lib.ab:1:"),
        "{said}"
    );
}

#[test]
fn testing_a_workspace_tests_every_member() {
    let test = Command::Test {
        path: ".".into(),
        only: None,
    };
    let (code, said) = at_the_top(
        &pair(
            "fn test_one(): null => null;",
            "fn test_other(): int => 1 / 0;",
        ),
        &test,
    );

    assert_eq!(code, 1);
    assert!(
        said.contains("ok    one:.test_one") && said.contains("FAIL  other:.test_other"),
        "{said}"
    );
}

#[test]
fn a_workspace_that_is_no_package_is_not_run() {
    let run = Command::Run {
        path: ".".into(),
        entry: "main.main".to_owned(),
        args: Vec::new(),
    };
    let said = "error: ambient.toml: this is a workspace's manifest, not a package's\n";
    assert_eq!(at_the_top(&pair(SEVEN, SEVEN), &run), (1, said.to_owned()));
}

/// The files once `app` is made to depend on the package at `on`,
/// under `name` if one is given, or what refused.
fn added(files: Files, on: &str, name: Option<&str>) -> Result<Files, String> {
    let command = Command::Add {
        on: PathBuf::from(on),
        name: name.map(str::to_owned),
        path: PathBuf::from("app"),
    };
    written(files, &command)
}

#[test]
fn adding_a_dependency_writes_what_the_package_says_of_itself() {
    let numbers = Package::versioned("kept/numbers", "n", "1.2.0").library(SEVEN);
    let files = added(files(None, [app(), numbers]), "kept/numbers", None).unwrap();

    let manifest = &files[&PathBuf::from("app/ambient.toml")];
    let written = "numbers = { id = \"n\", path = \"../kept/numbers\" }\n";
    assert!(manifest.ends_with(written), "{manifest}");
    assert_eq!(ran(&files), Ok(7));
}

#[test]
fn a_dependency_is_added_under_the_name_asked_for() {
    let app = package("app", "app").library(
        "use sevens.seven;
        pub fn main(): int => seven();",
    );
    let files = files(None, [app, package("numbers", "n").library(SEVEN)]);

    assert_eq!(
        added(files, "numbers", Some("sevens")).map(|files| ran(&files)),
        Ok(Ok(7))
    );
}

#[test]
fn a_dependency_is_not_added_over_another() {
    let app = app().on("numbers", "n", "path = \"../numbers\"");
    let files = files(None, [app, package("numbers", "n").library(SEVEN)]);

    assert_eq!(
        added(files, "numbers", None).err().as_deref(),
        Some("error: app/ambient.toml: there is a dependency `numbers` already\n")
    );
}

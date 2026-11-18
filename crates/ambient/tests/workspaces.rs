//! Packages that depend on each other, by directory and as members of
//! a workspace. See "Workspaces" in `spec/packages.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use support::scripted::run;

type Files = BTreeMap<PathBuf, String>;

/// A package to write out: where it is, and what its manifest says.
struct Package {
    at: &'static str,
    manifest: String,
    library: &'static str,
}

/// A package at `at`, named for it, with the ID `id` at version 1.0.0.
fn package(at: &'static str, id: &str) -> Package {
    Package::versioned(at, id, "1.0.0")
}

impl Package {
    fn versioned(at: &'static str, id: &str, version: &str) -> Self {
        let name = at.rsplit('/').next().unwrap();
        let manifest =
            format!("name = \"{name}\"\nid = \"{id}\"\nversion = \"{version}\"\n[dependencies]\n");
        Self {
            at,
            manifest,
            library: "",
        }
    }

    /// Depends on the package `id` as `name`, `found` saying where it
    /// is or which version: `path = ".."` or `version = ".."`.
    fn on(mut self, name: &str, id: &str, found: &str) -> Self {
        let dependency = format!("{name} = {{ id = \"{id}\", {found} }}\n");
        self.manifest.push_str(&dependency);
        self
    }

    /// Has `source` for its library module.
    fn library(mut self, source: &'static str) -> Self {
        self.library = source;
        self
    }
}

/// The files of `packages`, of which `app` says its `main` in its
/// library module, under a workspace at the top that lists `members`.
fn files<const N: usize>(members: Option<&[&str]>, packages: [Package; N]) -> Files {
    let mut files = Files::new();
    if let Some(members) = members {
        let listed: Vec<String> = members.iter().map(|at| format!("\"{at}\"")).collect();
        let manifest = format!("members = [{}]", listed.join(", "));
        files.insert("ambient.toml".into(), manifest);
    }
    for package in packages {
        let at = PathBuf::from(package.at);
        let module = if package.at == "app" { "main" } else { "lib" };
        files.insert(at.join("ambient.toml"), package.manifest);
        files.insert(
            at.join(format!("src/{module}.ab")),
            package.library.to_owned(),
        );
    }
    files
}

/// What running `app` exits with, or the error it reports.
fn ran(files: &Files) -> Result<i32, String> {
    let (code, world) = run(files);
    if world.err.is_empty() {
        Ok(code)
    } else {
        Err(world.err)
    }
}

/// An `app` whose `main` gives what `seven` of the package it calls
/// `numbers` gives.
fn app() -> Package {
    package("app", "app").library(
        "import numbers.seven;
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
        .library("import n.seven; pub fn fourteen(): int => seven() + seven();");
    let app = app()
        .on("numbers", "n", "path = \"../numbers\"")
        .on("twice", "t", "path = \"../twice\"")
        .library(
            "import numbers.seven; import twice.fourteen;
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
            "import odd.is_odd;
            pub fn is_even(n: int): bool {
                if n == 0 { return true; }
                return is_odd(n - 1);
            }",
        );
    let odd = package("odd", "o")
        .on("even", "e", "path = \"../even\"")
        .library(
            "import even.is_even;
            pub fn is_odd(n: int): bool {
                if n == 0 { return false; }
                return is_even(n - 1);
            }",
        );
    let app = package("app", "app")
        .on("even", "e", "path = \"../even\"")
        .library(
            "import even.is_even;
            pub fn main(): int {
                if is_even(10) { return 1; }
                return 0;
            }",
        );

    assert_eq!(ran(&files(None, [app, even, odd])), Ok(1));
}

#[test]
fn two_majors_of_a_package_are_two_members_under_two_names() {
    let old = Package::versioned("old", "n", "1.4.0").library(SEVEN);
    let new = Package::versioned("new", "n", "2.0.0").library("pub fn seventy(): int => 70;");
    let app = package("app", "app")
        .on("numbers", "n", "version = \"1.2.0\"")
        .on("numbers_next", "n", "version = \"2.0.0\"")
        .library(
            "import numbers.seven; import numbers_next.seventy;
            pub fn main(): int => seven() + seventy();",
        );
    let files = files(Some(&["app", "old", "new"]), [app, old, new]);

    assert_eq!(ran(&files), Ok(77));
}

#[test]
fn two_members_of_one_package_and_major_are_refused() {
    let files = files(
        Some(&["app", "one", "other"]),
        [
            app().on("numbers", "n", "version = \"1.0.0\""),
            package("one", "n").library(SEVEN),
            Package::versioned("other", "n", "1.3.0").library(SEVEN),
        ],
    );

    assert_eq!(
        ran(&files),
        Err("error: two members are package `n` at major 1: one and other\n".to_owned())
    );
}

#[test]
fn a_dependency_no_member_stands_in_for_is_refused() {
    let numbers = Package::versioned("numbers", "n", "2.0.0").library(SEVEN);
    let files = files(
        Some(&["app", "numbers"]),
        [app().on("numbers", "n", "version = \"1.0.0\""), numbers],
    );

    let said = "error: app: the dependency `numbers` is on package `n` at major 1, \
                which no member of the workspace is\n";
    assert_eq!(ran(&files), Err(said.to_owned()));
}

#[test]
fn a_dependency_is_held_to_what_the_package_at_its_directory_says() {
    let run = |declared: &str, version: &str| {
        let found = format!("path = \"../numbers\"{version}");
        let numbers = Package::versioned("numbers", "n", "1.2.0").library(SEVEN);
        ran(&files(
            None,
            [app().on("numbers", declared, &found), numbers],
        ))
    };
    let said = |declared: &str, found: &str| {
        Err(format!(
            "error: app: the dependency `numbers` says {declared}, \
             and the package at numbers is {found}\n"
        ))
    };

    assert_eq!(run("n", ", version = \"1.2.0\""), Ok(7));
    assert_eq!(run("m", ""), said("package `m`", "`n`"));
    assert_eq!(
        run("n", ", version = \"1.3.0\""),
        said("at least version 1.3.0", "version 1.2.0")
    );
    assert_eq!(
        run("n", ", version = \"2.0.0\""),
        said("at least version 2.0.0", "version 1.2.0")
    );
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

//! Binaries: the functions of `main` a package offers to be run. See
//! "Binaries" in `docs/spec/packages.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::path::PathBuf;

use ambient::cli::Command;
use support::scripted::{Scripted, declaring};

/// A `main` module of two functions that could each be a program.
const TWO: &str = "fn main(): int => 3;\nfn deploy(): int => 4;";

/// What running `bin` of a package whose manifest declares `declared`,
/// and whose `main` module is `source`, exits with and reports.
fn ran(declared: &str, source: &str, bin: &str) -> (i32, String) {
    let command = Command::Run {
        path: PathBuf::from("app"),
        bin: bin.to_owned(),
        args: Vec::new(),
    };
    let (code, world) = Scripted::default().execute(&declaring(source, declared), &command);
    (code, world.err)
}

/// What is reported of running `main` of such a package, after the
/// manifest it is reported of.
fn refused(declared: &str, source: &str) -> String {
    let (_, said) = ran(declared, source, "main");
    let said = said.trim_end().strip_prefix("error: app/ambient.toml: ");
    said.unwrap_or_default().to_owned()
}

#[test]
fn a_package_whose_main_defines_main_has_that_binary_undeclared() {
    assert_eq!(ran("", TWO, "main"), (3, String::new()));
}

#[test]
fn a_declared_binary_is_the_function_of_main_it_is_named_for() {
    assert_eq!(ran("[bin.deploy]", TWO, "deploy"), (4, String::new()));
}

#[test]
fn a_function_no_manifest_declares_is_not_run() {
    let (code, said) = ran("", TWO, "deploy");

    let lacks = "error: the package `app` has no binary `deploy`: \
                 its manifest declares no `[bin.deploy]`\n";
    assert_eq!((code, said.as_str()), (1, lacks));
}

#[test]
fn a_package_whose_main_defines_no_main_has_no_such_binary() {
    let (code, said) = ran("", "fn deploy(): int => 4;", "main");

    let lacks = "error: the package `app` has no binary `main`: \
                 `src/main.ab` defines no `main`\n";
    assert_eq!((code, said.as_str()), (1, lacks));
}

#[test]
fn a_declared_binary_that_main_does_not_define_is_refused() {
    assert_eq!(
        refused("[bin.absent]", TWO),
        "the binary `absent` is no function of `src/main.ab`"
    );
}

#[test]
fn two_binaries_whose_executables_are_named_the_same_are_refused() {
    assert_eq!(
        refused("[bin.deploy]\nname = \"main\"", TWO),
        "the binaries `deploy` and `main` are both named `main`"
    );
}

#[test]
fn an_executable_is_named_as_its_binary_says() {
    let built = |declared: &str| {
        let command = Command::Build {
            path: PathBuf::from("app"),
            bin: "deploy".to_owned(),
            output: None,
        };
        let files = declaring(TWO, declared);
        Scripted::default().execute(&files, &command).1.out
    };

    assert_eq!(built("[bin.deploy]"), "built deploy\n");
    assert_eq!(built("[bin.deploy]\nname = \"ship-it\""), "built ship-it\n");
}

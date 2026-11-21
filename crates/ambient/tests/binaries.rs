//! Binaries: the functions of `main` a package offers to be run. See
//! "Binaries" in `docs/spec/packages.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::path::PathBuf;

use ambient::cli::{Chosen, Command};
use support::scripted::{Scripted, declaring};

/// A `main` module of two functions that could each be a program.
const TWO: &str = "fn main(): int => 3;\nfn deploy(): int => 4;";

/// What running `bin` of a package whose manifest declares `declared`,
/// and whose `main` module is `source`, exits with and reports.
fn ran(declared: &str, source: &str, bin: &str) -> (i32, String) {
    ran_told(declared, source, Some(bin))
}

/// The same, of a command told `bin`: a binary, or none.
fn ran_told(declared: &str, source: &str, bin: Option<&str>) -> (i32, String) {
    let command = Command::Run {
        at: Chosen::default(),
        bin: bin.map(str::to_owned),
        args: Vec::new(),
    };
    let (code, world) = Scripted::within("app").execute(&declaring(source, declared), &command);
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

    let lacks = "error: no binary here is called `deploy`. There are:\n  app: main\n";
    assert_eq!((code, said.as_str()), (1, lacks));
}

#[test]
fn a_package_whose_main_defines_no_main_has_no_such_binary() {
    let (code, said) = ran("", "fn deploy(): int => 4;", "main");

    assert_eq!(code, 1);
    assert!(
        said.starts_with("error: there is no binary here: "),
        "{said}"
    );
}

#[test]
fn a_command_told_no_binary_runs_the_only_one_or_main() {
    let only = "fn deploy(): int => 4;";

    assert_eq!(ran_told("[bin.deploy]", only, None), (4, String::new()));
    assert_eq!(ran_told("[bin.deploy]", TWO, None), (3, String::new()));
}

#[test]
fn a_command_that_cannot_tell_which_binary_says_what_there_is() {
    let both = "fn deploy(): int => 4;\nfn publish(): int => 5;";
    let (code, said) = ran_told("[bin.deploy]\n[bin.publish]", both, None);

    let which = "error: say which binary with `--bin`. There are:\n  app: deploy, publish\n";
    assert_eq!((code, said.as_str()), (1, which));
}

#[test]
fn a_command_told_a_binary_of_no_name_says_what_there_is() {
    let (code, said) = ran("[bin.deploy]", TWO, "");

    let takes = "error: `--bin` takes the name of a binary. There are:\n  app: deploy, main\n";
    assert_eq!((code, said.as_str()), (1, takes));
}

/// What building the binary `bin`, or every one, of a package that
/// declares `declared` says, with `output` for where to put it.
fn built(declared: &str, bin: Option<&str>, output: Option<&str>) -> String {
    let command = Command::Build {
        at: Chosen::default(),
        bin: bin.map(str::to_owned),
        output: output.map(PathBuf::from),
    };
    let (_, world) = Scripted::within("app").execute(&declaring(TWO, declared), &command);
    world.out + &world.err
}

#[test]
fn building_told_no_binary_builds_every_one() {
    assert_eq!(
        built("[bin.deploy]", None, None),
        "built app/build/deploy\nbuilt app/build/main\n"
    );
}

#[test]
fn several_binaries_are_not_built_into_one_place() {
    let several = "error: `--output` is where one binary is put: say which with `--bin`\n";

    assert_eq!(built("[bin.deploy]", None, Some("out")), several);
    assert_eq!(
        built("[bin.deploy]", Some("main"), Some("out")),
        "built out\n"
    );
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
    let built = |declared: &str| built(declared, Some("deploy"), None);

    assert_eq!(built("[bin.deploy]"), "built app/build/deploy\n");
    assert_eq!(
        built("[bin.deploy]\nname = \"ship-it\""),
        "built app/build/ship-it\n"
    );
}

//! The command line, over files and a world held in memory. See
//! `spec/packages.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use ambient::cli::{Command, Session};
use ambient::host::World;
use ambient::store::MemoryStore;
use support::scripted::Scripted;

type Files = BTreeMap<PathBuf, String>;

fn files<const N: usize>(files: [(&str, &str); N]) -> Files {
    let file = |(path, text): (&str, &str)| (PathBuf::from(path), text.to_owned());
    files.into_iter().map(file).collect()
}

/// What a command exits with, and the world it leaves.
fn execute(files: &Files, command: &Command) -> (i32, Scripted) {
    let scripted = Rc::new(RefCell::new(Scripted::default()));
    let world: Rc<RefCell<dyn World>> = scripted.clone();
    let store = MemoryStore::new();
    let session = Session {
        files,
        store: &store,
        world,
    };
    let code = session.execute(command);
    let after = scripted.borrow().clone();
    (code, after)
}

fn run(files: &Files) -> (i32, Scripted) {
    let command = Command::Run {
        path: PathBuf::from("app"),
        entry: "main.main".to_owned(),
        args: Vec::new(),
    };
    execute(files, &command)
}

fn check(files: &Files) -> (i32, Scripted) {
    let path = PathBuf::from("app");
    execute(files, &Command::Check { path })
}

/// A package `app` whose `main` module is `source`.
fn app(source: &str) -> Files {
    files([
        ("app/ambient.toml", "name = \"app\""),
        ("app/src/main.ab", source),
    ])
}

#[test]
fn a_package_is_its_manifest_and_a_module_for_each_file() {
    let files = files([
        (
            "app/ambient.toml",
            "name = \"app\"\n[dependencies]\nwords = { path = \"../lib\" }",
        ),
        (
            "app/src/main.ab",
            r"
            import ambient.console;
            import pkg.text.greeting;
            import words.loud;

            fn main(): int with console.print {
                console.print!(loud(greeting.hello()));
                return 3;
            }
            ",
        ),
        (
            "app/src/text/greeting.ab",
            r#"pub fn hello(): string => "hello";"#,
        ),
        ("lib/ambient.toml", "name = \"shout\""),
        (
            "lib/src/lib.ab",
            r#"pub fn loud(text: string): string => text ++ "!";"#,
        ),
    ]);

    let (code, world) = run(&files);
    assert_eq!(
        (code, world.out.as_str(), world.err.as_str()),
        (3, "hello!\n", "")
    );
}

#[test]
fn text_that_is_no_module_is_reported_where_it_stops() {
    let (code, world) = run(&app("fn main(): int {\n  return 1\n}"));

    assert_eq!(code, 1);
    assert_eq!(
        world.err,
        "error: app/src/main.ab:3:1: expected `;`, found `}`\n"
    );
}

#[test]
fn checking_reports_every_definition_that_is_wrong() {
    let source = r#"
        fn one(): int => "one";
        fn two(): int => missing();
        fn fine(): int => 3;
        type Broken = Missing;
    "#;

    let (code, world) = check(&app(source));
    let problems: Vec<&str> = world.err.lines().collect();
    assert_eq!(code, 1);
    assert_eq!(problems.len(), 3, "{problems:?}");
    assert!(
        problems
            .iter()
            .all(|line| line.starts_with("error: app/src/main.ab: app:main."))
    );

    assert_eq!(check(&app("fn fine(): int => 3;")).0, 0);
}

#[test]
fn a_fault_is_reported_with_the_functions_it_left() {
    let source = "
        fn divide(by: int): int => 10 / by;
        fn main(): int {
            let result = divide(0);
            return result;
        }
    ";

    let (code, world) = run(&app(source));
    assert_eq!(code, 1);
    assert_eq!(
        world.err,
        "error: divide_by_zero: an integer was divided by zero\n  in app:main.divide\n  in app:main.main\n"
    );
}

#[test]
fn a_directory_without_a_manifest_is_not_a_package() {
    let (code, world) = run(&files([("app/src/main.ab", "")]));

    assert_eq!(code, 1);
    assert_eq!(world.err, "error: app/ambient.toml: no such file\n");
}

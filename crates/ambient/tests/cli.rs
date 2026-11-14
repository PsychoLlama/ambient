//! The command line, over files and a world held in memory. See
//! `spec/packages.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use support::scripted::{app, check, run};

type Files = BTreeMap<PathBuf, String>;

fn files<const N: usize>(files: [(&str, &str); N]) -> Files {
    let file = |(path, text): (&str, &str)| (PathBuf::from(path), text.to_owned());
    files.into_iter().map(file).collect()
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
    assert_eq!(
        problems[0],
        "error: app/src/main.ab:2: app:main.one: expected `int`, found `\"one\"`"
    );
    assert!(problems[1].starts_with("error: app/src/main.ab:3: app:main.two: "));
    assert!(problems[2].starts_with("error: app/src/main.ab:5: app:main.Broken: "));

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

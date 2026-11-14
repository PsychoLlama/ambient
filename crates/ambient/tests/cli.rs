//! The command line, over files and a world held in memory. See
//! `spec/packages.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use support::scripted::{Scripted, app, check, run};

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
        "error: app/src/main.ab:2:26: app:main.one: expected `int`, found `\"one\"`"
    );
    assert!(problems[1].starts_with("error: app/src/main.ab:3:26: app:main.two: "));
    assert!(problems[2].starts_with("error: app/src/main.ab:5:9: app:main.Broken: "));

    assert_eq!(check(&app("fn fine(): int => 3;")).0, 0);
}

/// The line and column checking reports for a module whose one wrong
/// definition is `source`, written from the first column of line one.
fn wrong_at(source: &str) -> String {
    let (_, world) = check(&app(source));
    let place = world.err.strip_prefix("error: app/src/main.ab:");
    let place = place.and_then(|rest| rest.split(": ").next());
    place.unwrap_or(&world.err).to_owned()
}

#[test]
fn an_error_is_reported_at_the_expression_that_is_wrong() {
    // An argument, an operand's operator, and a name that is not there.
    assert_eq!(
        wrong_at("fn f(n: int): int => n;\nfn g(): int => f(\"a\");"),
        "2:18"
    );
    assert_eq!(wrong_at("fn f(): int => 1 + \"a\";"), "1:18");
    assert_eq!(
        wrong_at("fn f(): int {\n  let a = 1;\n  return a + b;\n}"),
        "3:14"
    );
    // What a let says it binds, at the value that is not that.
    assert_eq!(
        wrong_at("fn f(): int {\n  let a: int = \"a\";\n  return a;\n}"),
        "2:16"
    );
    // A method the value lacks, at its name.
    assert_eq!(wrong_at("fn f(): int => 1\n  .missing();"), "2:4");
}

#[test]
fn a_name_that_is_wrong_is_reported_where_it_is_wrong() {
    // The ability is performed on the line before, and that is fine.
    let source = "ability say(text: string): null;
fn f(): null with say {
  say!(\"a\");
  say(\"b\");
}";
    let (_, world) = check(&app(source));

    assert_eq!(
        world.err,
        "error: app/src/main.ab:4:3: app:main.f: `say` is an ability, which is performed: `say!(...)`\n"
    );
}

#[test]
fn what_a_block_gives_is_wrong_at_the_return_that_gives_it() {
    let after = "fn f(): int {\n  let a = 1;\n  return \"a\";\n}";
    let branch = "fn f(n: int): int {\n  if n > 0 {\n    return \"a\";\n  }\n  return 1;\n}";
    let held = "fn f(): int {\n  using a = 1;\n  return 2;\n}";

    assert_eq!(wrong_at(after), "3:10");
    assert_eq!(wrong_at(branch), "3:12");
    // What a using binds holds no cleanup: at the statement.
    assert_eq!(wrong_at(held), "2:3");
}

#[test]
fn an_error_inside_a_lambda_is_at_its_place_in_the_definition() {
    let source = "fn f(): List<int> =>\n  [1].map(fn(n) =>\n    n ++ 1);";

    assert_eq!(wrong_at(source), "3:7");
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
fn an_error_nothing_catches_ends_the_program_and_is_reported() {
    let source = r#"
        import ambient.{catch, fail, throw};
        tag too_big;

        fn small(n: int): int with throw {
            if n > 9 { fail(too_big, "${n} is not small"); }
            return n;
        }
        fn main(): int with throw {
            let caught = catch(fn() => small(10));
            let first = caught as int else small(3);
            return first + small(12);
        }
    "#;

    let (code, world) = run(&app(source));
    assert_eq!(code, 1);
    assert_eq!(
        world.err,
        "error: main.too_big: 12 is not small\n  in ambient:errors.fail\n  in app:main.small\n  in app:main.main\n"
    );
}

#[test]
fn a_directory_without_a_manifest_is_not_a_package() {
    let (code, world) = run(&files([("app/src/main.ab", "")]));

    assert_eq!(code, 1);
    assert_eq!(world.err, "error: app/ambient.toml: no such file\n");
}

fn test(files: &Files) -> (i32, Scripted) {
    let path = PathBuf::from("app");
    Scripted::default().execute(files, &ambient::cli::Command::Test { path })
}

#[test]
fn a_test_is_a_function_that_takes_nothing_and_says_what_holds() {
    let source = r#"
        import ambient.test.{check};

        fn double(n: int): int => n * 2;

        fn test_doubles(): null with check {
            check!(double(2) == 4, "two doubles to four");
        }
        fn test_is_wrong(): null with check {
            check!(double(2) == 5, "two doubles to five");
            check!(double(0) == 1, "nothing doubles to one");
        }
        fn test_faults(): int => 1 / (double(0));
        fn test_takes(n: int): int => n;
    "#;
    let (code, world) = test(&app(source));

    assert_eq!(code, 1);
    assert_eq!(
        world.out,
        "ok    app:main.test_doubles
FAIL  app:main.test_is_wrong
      does not hold: two doubles to five
      does not hold: nothing doubles to one
FAIL  app:main.test_faults
      divide_by_zero: an integer was divided by zero
        in app:main.test_faults
1 passed, 2 failed
"
    );
}

#[test]
fn a_test_that_throws_fails_with_what_it_threw() {
    let source = r#"
        import ambient.{fail, throw};
        tag unready;
        fn test_throws(): null with throw { fail(unready, "not yet"); }
    "#;
    let (code, world) = test(&app(source));

    assert_eq!(code, 1);
    assert_eq!(
        world.out,
        "FAIL  app:main.test_throws
      main.unready: not yet
        in ambient:errors.fail
        in app:main.test_throws
0 passed, 1 failed
"
    );
}

#[test]
fn a_test_reaches_no_world_it_is_not_given() {
    let source = r#"
        import ambient.console;
        import ambient.test.{check};

        fn greet(): null with console.print { console.print!("hello"); }

        fn test_reaches(): null with console.print { greet(); }
        fn test_provides(): null with check {
            let said = cell<string>("");
            provide { [console.print](text) { said.update(fn(_) => text); } } { greet(); }
            check!(said.read() == "hello", "it greets");
        }
    "#;
    let (code, world) = test(&app(source));

    assert_eq!(code, 1);
    let refused = "`ambient:console.print` is not bound here\n";
    let (failed, rest) = world.out.split_once(refused).unwrap_or_default();
    assert!(
        failed.starts_with("FAIL  app:main.test_reaches\n"),
        "{failed}"
    );
    assert_eq!(rest, "ok    app:main.test_provides\n1 passed, 1 failed\n");
}

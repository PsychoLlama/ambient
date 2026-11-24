//! The command line, over files and a world held in memory. See
//! `docs/spec/packages.md`.

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
            "[package]\nname = \"app\"\nid = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nwords = { path = \"../lib\" }",
        ),
        (
            "app/src/main.ab",
            r"
            use system.console;
            use pkg.text.greeting;
            use words.loud;

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
        (
            "lib/ambient.toml",
            "[package]\nname = \"shout\"\nid = \"shout\"\nversion = \"0.1.0\"",
        ),
        (
            "lib/src/lib.ab",
            r#"pub fn loud(text: string): string => text + "!";"#,
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

/// `os` binds `throw` to a native handler, which takes the trace.
#[test]
fn a_target_that_extends_the_command_lines_is_checked() {
    let (code, world) = check(&app("use system.{os};\npub target world extends os { }"));

    assert_eq!((code, world.err.as_str()), (0, ""));
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
fn what_a_module_cannot_hold_is_reported_where_it_is_written() {
    let import = "fn one(): int => 1;\nuse system.{os, os};";
    let twice = "fn one(): int => 1;\n\nfn one(): int => 2;";

    assert_eq!(wrong_at(import), "2:1");
    assert_eq!(wrong_at(twice), "3:1");
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
    // A test that cannot tell what it is asked: any record passes it.
    assert_eq!(
        wrong_at(
            "use ambient.map.{Entry, Map};\nfn f(v: unknown): int =>\n  cast::<int>(cast::<Map<Entry<string, int>>>(v, null), 0);"
        ),
        "3:15"
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
    let source = "fn f(): List<int> =>\n  [1].map(fn(n) =>\n    n + \"a\");";

    assert_eq!(wrong_at(source), "3:7");
}

#[test]
fn an_ability_unbound_in_a_function_written_in_place_says_what_to_write() {
    let source = "use ambient.errors.throw;\nfn risky(): int with throw => 1;\n\
                  fn main(): int {\n  let held = fn(): int => risky();\n  return 0;\n}";
    let (_, world) = check(&app(source));

    let said = world.err.trim_end().split("here: ").nth(1);
    let hint = "write `with throw` on this function for what calls it to bind";
    assert_eq!(said, Some(hint), "{}", world.err);
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
        "error: divide_by_zero: an integer was divided by zero\n  in app:main.divide at app/src/main.ab:2:39\n  in app:main.main at app/src/main.ab:4:26\n"
    );
}

#[test]
fn a_frame_in_a_lambda_is_where_the_lambda_is_written() {
    let source = "fn main(): int =>\n  [1, 0].map(fn(n) =>\n    10 / n).sum();";

    let (_, world) = run(&app(source));
    let frames: Vec<&str> = world.err.lines().skip(1).collect();
    assert_eq!(
        frames.first(),
        Some(&"  in app:main.main#0 at app/src/main.ab:3:8")
    );
    assert_eq!(
        frames.last(),
        Some(&"  in app:main.main at app/src/main.ab:2:10")
    );
}

/// A module with a function `made` of one integer, generated with the
/// body `body`, and what follows it.
fn generating(body: &str, rest: &str) -> String {
    format!(
        "use ambient.code;
use ambient.tree;
fn make(): tree.Function =>
    code.define([tree.integer], tree.integer, [], fn(given: List<code.Code>): code.Code =>
        {body});
fn made = comptime make();
{rest}"
    )
}

#[test]
fn an_error_in_generated_code_is_at_the_call_with_the_code() {
    let sum = "code.operate(\"add\", [code.literal(1), code.literal(\"two\")])";

    let (code, world) = check(&app(&generating(sum, "")));
    assert_eq!(code, 1);
    assert_eq!(
        world.err,
        "error: app/src/main.ab:6:1: app:main.made: `+` is not defined for `1, \"two\"`
in the code the call gave:
    fn made(_0: int): int => 1 + \"two\"
                             ^^^^^^^^^
"
    );
}

#[test]
fn generated_code_is_written_out_as_it_would_be_read() {
    let cast = "code.cast(code.literal(1), tree.integer, code.literal(0))";
    let joined = format!("code.operate(\"join\", [{cast}, code.literal(\"two\")])");

    let (_, world) = check(&app(&generating(&joined, "")));
    assert_eq!(
        world.err,
        "error: app/src/main.ab:6:1: app:main.made: `intrinsics.join` is not defined for `int, \"two\"`
in the code the call gave:
    fn made(_0: int): int => intrinsics.join(cast::<int>(1, 0), \"two\")
                             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
"
    );
}

#[test]
fn a_generated_cast_whose_other_branch_does_something_is_written_as_a_match() {
    let sum = "code.operate(\"add\", [code.literal(1), code.literal(\"two\")])";
    let cast = format!("code.cast(code.literal(1), tree.integer, {sum})");

    let (_, world) = check(&app(&generating(&cast, "")));
    assert_eq!(
        world.err,
        "error: app/src/main.ab:6:1: app:main.made: `+` is not defined for `1, \"two\"`
in the code the call gave:
    fn made(_0: int): int => match 1 {
        let passed: int => passed,
        _ => 1 + \"two\",
             ^^^^^^^^^
    }
"
    );
}

#[test]
fn a_frame_in_generated_code_is_where_the_call_is() {
    let quotient = "code.operate(\"divide\", [code.literal(10), given.first() ?? code.literal(1)])";

    let (_, world) = run(&app(&generating(quotient, "fn main(): int => made(0);")));
    assert_eq!(
        world.err,
        "error: divide_by_zero: an integer was divided by zero\n  in app:main.made at app/src/main.ab:6:1\n"
    );
}

#[test]
fn an_error_nothing_catches_ends_the_program_and_is_reported() {
    let source = r#"
        use ambient.{catch, fail, throw};
        tag too_big;

        fn small(n: int): int with throw {
            if n > 9 { fail(too_big, "${n} is not small"); }
            return n;
        }
        fn main(): int with throw {
            let caught = catch(fn() => small(10));
            let first = cast::<int>(caught, null) ?? small(3);
            return first + small(12);
        }
    "#;

    let (code, world) = run(&app(source));
    assert_eq!(code, 1);
    assert_eq!(
        world.err,
        "error: main.too_big: 12 is not small\n  in ambient:errors.fail\n  in app:main.small at app/src/main.ab:6:24\n  in app:main.main at app/src/main.ab:12:28\n"
    );
}

#[test]
fn a_directory_without_a_manifest_is_not_a_package() {
    let (code, world) = run(&files([("app/src/main.ab", "")]));

    assert_eq!(code, 1);
    assert_eq!(
        world.err,
        "error: app: there is no `ambient.toml` here or above\n"
    );
}

fn test(files: &Files) -> (i32, Scripted) {
    testing(files, None)
}

/// Runs the tests of `files` whose names hold `only`, or all of them.
fn testing(files: &Files, only: Option<&str>) -> (i32, Scripted) {
    let only = only.map(str::to_owned);
    Scripted::within("app").execute(
        files,
        &ambient::cli::Command::Test {
            at: ambient::cli::Chosen::default(),
            only,
        },
    )
}

#[test]
fn a_test_is_a_function_that_takes_nothing_and_says_what_holds() {
    let source = r#"
        use ambient.test;
        use ambient.test.{check, equal, unmet};

        fn double(n: int): int => n * 2;

        #[test.case]
        fn test_doubles(): null with unmet {
            check(double(2) == 4, "two doubles to four");
        }
        #[test.case]
        fn test_is_wrong(): null with unmet {
            check(double(2) == 5, "two doubles to five");
            equal(double(0), 1, "nothing doubles to one");
        }
        #[test.case]
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
        found: 0
        expected: 1
FAIL  app:main.test_faults
      divide_by_zero: an integer was divided by zero
        in app:main.test_faults at app/src/main.ab:17:36
1 passed, 2 failed
"
    );
}

#[test]
fn a_test_a_generated_module_was_given_is_run() {
    let manifest = "[package]\nname = \"app\"\nid = \"app\"\nversion = \"0.1.0\"";
    let file = |(path, text): (&str, &str)| (PathBuf::from(path), text.to_owned());
    let files = [
        ("app/ambient.toml", manifest),
        (
            "app/src/make.ab",
            "use ambient.code; use ambient.tree; use ambient.test; use ambient.test.{unmet};
            pub fn all(): tree.Module => [code.said(code.named(\"made\", code.define(
                [], tree.nil, [comptime tree.reference(unmet)], fn(given: List<code.Code>): code.Code =>
                    code.perform(comptime tree.reference(unmet), [code.record([(\"what\", code.literal(\"made\"))])]))),
                [{ key: comptime tree.reference(test.case), value: null }])];",
        ),
        ("app/src/made.ab", "use pkg.make; comptime make.all();"),
    ];
    let (code, world) = test(&files.into_iter().map(file).collect());

    assert_eq!(code, 1);
    assert!(world.out.contains("FAIL  app:made.made"), "{}", world.out);
}

#[test]
fn a_test_that_throws_fails_with_what_it_threw() {
    let source = r#"
        use ambient.{fail, throw, test};
        tag unready;
        #[test.case]
        fn test_throws(): null with throw { fail(unready, "not yet"); }
    "#;
    let (code, world) = test(&app(source));

    assert_eq!(code, 1);
    assert_eq!(
        world.out,
        "FAIL  app:main.test_throws
      main.unready: not yet
        in ambient:errors.fail
        in app:main.test_throws at app/src/main.ab:5:45
0 passed, 1 failed
"
    );
}

#[test]
fn a_test_is_not_run_again_until_it_changes_whether_it_passed_or_not() {
    let source = "
        use ambient.test;
        fn two(): int => 2;
        #[test.case]
        fn test_holds(): int => two();
        #[test.case]
        fn test_faults(): int => 1 / 0;
    ";
    let store = ambient::store::MemoryStore::new();
    // What a run keeps it keeps in its world, which the next finds.
    let world = std::cell::RefCell::new(Scripted::within("app"));
    let summary = |source: &str| {
        let command = ambient::cli::Command::Test {
            at: ambient::cli::Chosen::default(),
            only: None,
        };
        let (_, after) = world
            .take()
            .again()
            .execute_over(&app(source), &store, &command);
        let said = after.out.lines().last().unwrap().to_owned();
        world.replace(after);
        said
    };

    assert_eq!(summary(source), "1 passed, 1 failed");
    // Renaming changes no hash; what a test calls changing does.
    let renamed = source.replace("two", "pair");
    assert_eq!(summary(&renamed), "1 passed (2 not run again), 1 failed");
    // Only the test that reaches what changed runs again.
    let changed = source.replace("=> 2", "=> 3");
    assert_eq!(summary(&changed), "1 passed (1 not run again), 1 failed");
}

#[test]
fn a_test_moved_unchanged_is_not_run_again_and_is_named_where_it_is() {
    let manifest = "[package]\nname = \"app\"\nid = \"app\"\nversion = \"0.1.0\"";
    let package = |module: &str| -> Files {
        let test = "use ambient.test;\nfn split(n: int): int => n / 0;\n#[test.case]\nfn test_faults(): int => split(1) + 1;";
        [("app/ambient.toml", manifest), (module, test)]
            .into_iter()
            .map(|(path, text)| (PathBuf::from(path), text.to_owned()))
            .collect()
    };
    let store = ambient::store::MemoryStore::new();
    let command = ambient::cli::Command::Test {
        at: ambient::cli::Chosen::default(),
        only: None,
    };
    let world = std::cell::RefCell::new(Scripted::within("app"));
    let tested = |files: &Files| {
        let (_, after) = world.take().again().execute_over(files, &store, &command);
        let said = after.out.clone();
        world.replace(after);
        said
    };

    tested(&package("app/src/spec.ab"));
    let moved = tested(&package("app/src/sensing.ab"));

    assert_eq!(
        moved,
        "FAIL  app:sensing.test_faults
      divide_by_zero: an integer was divided by zero
        in app:sensing.split at app/src/sensing.ab:2:28
        in app:sensing.test_faults at app/src/sensing.ab:4:26
0 passed (1 not run again), 1 failed
"
    );
}

#[test]
fn only_the_tests_asked_for_are_run() {
    let source = "
        use ambient.test;
        #[test.case]
        fn test_one(): null => null;
        #[test.case]
        fn test_two(): int => 1 / 0;
    ";

    let (code, world) = testing(&app(source), Some("main.test_o"));
    assert_eq!(
        (code, world.out.as_str()),
        (0, "ok    app:main.test_one\n1 passed, 0 failed\n")
    );
}

#[test]
fn a_test_reaches_what_the_system_package_binds() {
    let source = r#"
        use system.exchange;
        use ambient.test;
        use ambient.test.{check, unmet};

        #[test.case]
        fn test_reads_back(): null with unmet, exchange {
            let sent = cast::<bytes>(exchange.encode!([1, 2]), "".to_bytes());
            let read = cast::<List<int>>(exchange.decode!(sent), []);
            check(read == [1, 2], "a value is read back");
        }
    "#;
    let (code, world) = test(&app(source));

    assert_eq!((code, world.err.as_str()), (0, ""), "{}", world.out);
}

#[test]
fn a_test_reaches_the_world_its_row_names_and_runs_every_time() {
    let source = r#"
        use system.console;
        use ambient.test;
        use ambient.test.{check, unmet};

        fn greet(): null with console.print { console.print!("hello"); }

        #[test.case]
        fn test_reaches(): null with console.print { greet(); }
        #[test.case]
        fn test_provides(): null with unmet {
            let said = cell::<string>("");
            provide { [console.print](text) { said.update(fn(_) => text); } } { greet(); }
            check(said.read() == "hello", "it greets");
        }
    "#;
    let store = ambient::store::MemoryStore::new();
    let world = std::cell::RefCell::new(Scripted::within("app"));
    let tested = || {
        let command = ambient::cli::Command::Test {
            at: ambient::cli::Chosen::default(),
            only: None,
        };
        let (code, after) = world
            .take()
            .again()
            .execute_over(&app(source), &store, &command);
        let out = after.out.clone() + &after.err;
        world.replace(after);
        (code, out)
    };

    let first =
        "hello\nok    app:main.test_reaches\nok    app:main.test_provides\n2 passed, 0 failed\n";
    assert_eq!(tested(), (0, first.to_owned()));
    // What reaches the world is never kept; what provides it is.
    let again = first.replace("2 passed,", "2 passed (1 not run again),");
    assert_eq!(tested(), (0, again));
}

/// Makes a package at `path` in `world`.
fn made(path: &str, world: Scripted) -> (i32, Scripted) {
    made_as(path, world, false)
}

/// Makes a package at `path` in `world`: a library, or a program.
fn made_as(path: &str, world: Scripted, library: bool) -> (i32, Scripted) {
    let path = PathBuf::from(path);
    let init = ambient::cli::Command::Init {
        path,
        bin: false,
        lib: library,
    };
    world.execute(&Files::new(), &init)
}

/// What testing the package a command made in `world` gives.
fn tested(world: &Scripted, path: &str) -> (i32, Scripted) {
    let held = |bytes: &Vec<u8>| String::from_utf8_lossy(bytes).into_owned();
    let written = |(path, bytes): (&String, &Vec<u8>)| (PathBuf::from(path), held(bytes));
    let files: Files = world.files.iter().map(written).collect();
    let test = ambient::cli::Command::Test {
        at: ambient::cli::Chosen::default(),
        only: None,
    };
    Scripted::within(path).execute(&files, &test)
}

#[test]
fn a_new_package_runs_and_its_test_passes() {
    let (code, world) = made("work/app", Scripted::default());
    assert_eq!(
        (code, world.out.as_str()),
        (0, "made work/app: `ambient run` there runs it\n")
    );

    let (code, world) = tested(&world, "work/app");
    assert_eq!((code, world.err.as_str()), (0, ""), "{}", world.out);
}

#[test]
fn a_new_library_has_a_library_module_and_its_test_passes() {
    let (code, world) = made_as("work/lib", Scripted::default(), true);
    assert_eq!(
        (code, world.out.as_str()),
        (0, "made work/lib: `ambient test` there tests it\n")
    );
    assert!(world.files.contains_key("work/lib/src/lib.ab"));
    // A library builds nothing to keep out of a repository.
    assert!(!world.files.contains_key("work/lib/.gitignore"));

    let (code, world) = tested(&world, "work/lib");
    assert_eq!((code, world.err.as_str()), (0, ""), "{}", world.out);
}

#[test]
fn a_new_package_is_named_for_its_directory_and_given_an_id() {
    let world = Scripted {
        directory: Some("/work/tea".to_owned()),
        files: [("./src/main.ab".to_owned(), "kept".into())].into(),
        ..Scripted::default()
    };
    let (code, world) = made(".", world);

    assert_eq!((code, world.err.as_str()), (0, ""));
    let id = format!("{:016x}{:016x}", 1, 2);
    assert_eq!(
        world.file("./ambient.toml"),
        format!(
            "[package]\nname = \"tea\"\nid = \"{id}\"\nversion = \"0.1.0\"\n\n\
             [bin.main]\nname = \"tea\"\n"
        )
    );
    assert_eq!(world.file("./.gitignore"), "/build\n");
    // What was there already is left as it was.
    assert_eq!(world.file("./src/main.ab"), "kept");
}

#[test]
fn a_package_is_not_made_where_it_cannot_be() {
    let (code, world) = made("work/my-app", Scripted::default());
    assert_eq!(code, 1);
    assert!(
        world
            .err
            .starts_with("error: work/my-app: a package is named")
    );

    let world = Scripted {
        files: [("app/ambient.toml".to_owned(), Vec::new())].into(),
        ..Scripted::default()
    };
    let (_, world) = made("app", world);
    assert_eq!(world.err, "error: app: there is a package here already\n");
}

#[test]
fn a_package_says_an_id_that_is_its_own() {
    let refused = |manifest: &str| {
        let files = files([("app/ambient.toml", manifest), ("app/src/main.ab", "")]);
        check(&files).1.err
    };

    assert_eq!(
        refused("[package]\nname = \"app\""),
        "error: app/ambient.toml: a package's manifest says its `id`\n"
    );
    assert_eq!(
        refused("[package]\nname = \"app\"\nid = \"ambient\"\nversion = \"0.1.0\""),
        "error: app/ambient.toml: a package's ID cannot be `ambient`\n"
    );
    assert_eq!(
        refused("[package]\nname = \"app\"\nid = \"system\"\nversion = \"0.1.0\""),
        "error: app/ambient.toml: a package's ID cannot be `system`\n"
    );
    assert_eq!(
        refused("[package]\nname = \"system\"\nid = \"app\"\nversion = \"0.1.0\""),
        "error: app/ambient.toml: a package cannot be named `system`\n"
    );
}

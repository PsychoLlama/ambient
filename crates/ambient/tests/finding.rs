//! Finding tests: the ways a manifest lists, or the standard library's.
//! See "Finding" in `docs/spec/testing.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use ambient::cli::{Chosen, Command};
use support::scripted::Scripted;

/// Tests a package whose manifest ends with `rest` and whose modules
/// are `modules`, each a path under `src` and its text, and gives the
/// code it exits with and what it says.
fn tested(rest: &str, modules: &[(&str, &str)]) -> (i32, String) {
    let manifest = format!("[package]\nname = \"app\"\nid = \"app\"\nversion = \"0.1.0\"\n{rest}");
    let mut files: BTreeMap<PathBuf, String> = modules
        .iter()
        .map(|(path, text)| (PathBuf::from(format!("app/src/{path}")), (*text).to_owned()))
        .collect();
    files.insert(PathBuf::from("app/ambient.toml"), manifest);
    let test = Command::Test {
        at: Chosen::default(),
        only: None,
    };
    let (code, world) = Scripted::within("app").execute(&files, &test);
    (code, world.out + &world.err)
}

/// A way to find tests: every function of a module under `checks`,
/// named in a group for the directory.
const UNDER: &str = r#"
    use ambient.build;
    use ambient.build.{Module, Selection};
    use ambient.test.{Test, Tests};
    use ambient.tree.{Reference};

    pub fn under(of: Selection): Tests with build.definitions {
        let checks = of.modules.filter(fn(module: Module): bool => module.path.length() == 2 && module.path.get(0) == "checks");
        return checks.fold([], fn(tests: Tests, module: Module): Tests {
            let defined = cast::<List<Reference>>(build.definitions!(module), []);
            let group = { name: "checks", group: null };
            return tests.join(defined.map(fn(function: Reference): Test =>
                { name: function.name, group: { name: module.path.get(1) ?? "", group }, function }));
        });
    }
"#;

#[test]
fn a_package_finds_its_tests_its_own_way() {
    let (code, said) = tested(
        "[test]\nfind = [\"pkg.find.under\"]",
        &[
            ("find.ab", UNDER),
            (
                "checks/sums.ab",
                "fn adds(): int => 1 + 1;\nfn faults(): int => 1 / 0;",
            ),
            (
                "main.ab",
                "use ambient.test; #[test.case] fn elsewhere(): null => null;",
            ),
        ],
    );

    assert_eq!(code, 1);
    let lines: Vec<&str> = said.lines().filter(|line| !line.starts_with(' ')).collect();
    assert_eq!(
        lines,
        [
            "ok    checks.sums.adds",
            "FAIL  checks.sums.faults",
            "1 passed, 1 failed"
        ]
    );
}

#[test]
fn several_ways_find_a_test_once_where_it_was_first_found() {
    let (code, said) = tested(
        "[test]\nfind = [\"pkg.find.under\", \"ambient.test.annotated\"]",
        &[
            ("find.ab", UNDER),
            (
                "checks/sums.ab",
                "use ambient.test; #[test.case] fn adds(): int => 1 + 1;",
            ),
            (
                "main.ab",
                "use ambient.test; #[test.case] fn elsewhere(): null => null;",
            ),
        ],
    );

    assert_eq!(
        (code, said.as_str()),
        (
            0,
            "ok    checks.sums.adds\nok    app:main.elsewhere\n2 passed, 0 failed\n"
        )
    );
}

#[test]
fn a_way_to_find_tests_takes_a_selection_and_gives_tests() {
    let (code, said) = tested(
        "[test]\nfind = [\"pkg.main.wrong\"]",
        &[("main.ab", "fn wrong(): int => 1;")],
    );

    assert_eq!(code, 1);
    assert!(
        said.contains("a way to find tests takes a `build.Selection` and gives `test.Tests`"),
        "{said}"
    );
}

#[test]
fn a_test_that_takes_something_fails_and_runs_no_other_the_worse() {
    let (code, said) = tested(
        "",
        &[(
            "main.ab",
            "use ambient.test;
            #[test.case] fn takes(n: int): int => n;
            #[test.case] fn holds(): null => null;",
        )],
    );

    assert_eq!(
        (code, said.as_str()),
        (
            1,
            "FAIL  app:main.takes
      app/src/main.ab:2:26: app:main.takes: a test is a function that takes nothing
ok    app:main.holds
1 passed, 1 failed
"
        )
    );
}

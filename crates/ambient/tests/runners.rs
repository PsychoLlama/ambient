//! Running tests: the standard runner's choices, and a package's own.
//! See "Running" in `docs/spec/testing.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use ambient::cli::{Chosen, Command};
use ambient::store::MemoryStore;
use support::scripted::Scripted;

/// Tests a package whose manifest ends with `rest` and whose library
/// module is `source`, over `store`, and gives the code it exits with
/// and what it says.
fn tested(store: &MemoryStore, rest: &str, source: &str) -> (i32, String) {
    let manifest = format!("[package]\nname = \"app\"\nid = \"app\"\nversion = \"0.1.0\"\n{rest}");
    let files = BTreeMap::from([
        (PathBuf::from("app/ambient.toml"), manifest),
        (PathBuf::from("app/src/lib.ab"), source.to_owned()),
    ]);
    let test = Command::Test {
        at: Chosen::default(),
        only: None,
    };
    let (code, world) = Scripted::within("app").execute_over(&files, store, &test);
    (code, world.out + &world.err)
}

#[test]
fn a_failure_of_a_test_that_may_be_tried_again_is_never_reused() {
    let source = "use ambient.test;
        #[test.case] fn once(): int => 1 / 0;
        #[test.case] #[test.tries(2)] fn twice(): int => 2 / 0;";
    let store = MemoryStore::new();
    let summary = || {
        tested(&store, "", source)
            .1
            .lines()
            .last()
            .unwrap()
            .to_owned()
    };

    assert_eq!(summary(), "0 passed, 2 failed");
    assert_eq!(summary(), "0 passed (1 not run again), 2 failed");
}

/// A runner that reports only what failed, over the standard one.
const QUIET: &str = "use ambient.build;
    use ambient.test;
    use ambient.test.{Event, Outcome, Tests};

    pub fn quiet(tests: Tests): null with test.run, test.kept, test.report, build.annotations {
        provide {
            [test.report](event) {
                let said = match event {
                    let ended: { outcome: Outcome } => test.passed(ended.outcome) ? null : event,
                    _ => event,
                };
                if said != null {
                    test.report!(event);
                }
            }
        } {
            test.tests(tests);
        }
    }

    #[test.case] fn holds(): null => null;
    #[test.case] fn faults(): int => 1 / 0;";

#[test]
fn a_package_runs_its_tests_its_own_way() {
    let (code, said) = tested(&MemoryStore::new(), "[test]\nrun = \"pkg.quiet\"", QUIET);
    let lines: Vec<&str> = said.lines().filter(|line| !line.starts_with(' ')).collect();

    assert_eq!(code, 1);
    assert_eq!(lines, ["FAIL  app.faults", "1 passed, 1 failed"]);
}

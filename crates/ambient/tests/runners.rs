//! Running tests: the standard runner's choices, and a package's own.
//! See "Running" and "Keeping" in `docs/spec/testing.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

use ambient::cli::{Chosen, Command};
use ambient::store::MemoryStore;
use support::scripted::Scripted;

/// Runs the tests of packages in one world, which keeps what each run
/// leaves for the next, over one store.
#[derive(Default)]
struct Runs {
    store: MemoryStore,
    world: RefCell<Scripted>,
}

impl Runs {
    /// Tests a package whose manifest ends with `rest` and whose library
    /// module is `source`, and gives the code it exits with and what it
    /// says.
    fn tested(&self, rest: &str, source: &str) -> (i32, String) {
        let manifest =
            format!("[package]\nname = \"app\"\nid = \"app\"\nversion = \"0.1.0\"\n{rest}");
        let files = BTreeMap::from([
            (PathBuf::from("app/ambient.toml"), manifest),
            (PathBuf::from("app/src/lib.ab"), source.to_owned()),
        ]);
        let test = Command::Test {
            at: Chosen::default(),
            only: None,
        };
        let world = self.world.take().again().inside("app");
        let (code, after) = world.execute_over(&files, &self.store, &test);
        let said = after.out.clone() + &after.err;
        self.world.replace(after);
        (code, said)
    }

    /// The last line of what testing `source` says.
    fn summary(&self, source: &str) -> String {
        self.tested("", source).1.lines().last().unwrap().to_owned()
    }
}

#[test]
fn a_failure_of_a_test_that_may_be_tried_again_is_never_reused() {
    let source = "use ambient.test;
        #[test.case] fn once(): int => 1 / 0;
        #[test.case] #[test.tries(2)] fn twice(): int => 2 / 0;";
    let runs = Runs::default();

    assert_eq!(runs.summary(source), "0 passed, 2 failed");
    assert_eq!(runs.summary(source), "0 passed (1 not run again), 2 failed");
}

/// A test that only computes is kept, though its row names native
/// code; one that reads the world is run every time.
#[test]
fn a_test_that_only_hashes_is_not_run_again() {
    let source = "use ambient.test;
        use system.{clock, crypto};
        #[test.case] fn hashes(): int with crypto => crypto.hash!(crypto.sha256, 0x[]).length();
        #[test.case] fn reads(): int with clock.now => clock.now!();";
    let runs = Runs::default();

    assert_eq!(runs.summary(source), "2 passed, 0 failed");
    assert_eq!(runs.summary(source), "2 passed (1 not run again), 0 failed");
}

#[test]
fn what_is_kept_is_kept_under_the_build_directory_by_package() {
    let runs = Runs::default();
    runs.tested(
        "",
        "use ambient.test; #[test.case] fn holds(): null => null;",
    );

    let world = runs.world.take();
    let kept: Vec<&String> = world.files.keys().collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(kept[0].starts_with("app/build/tests/app/"), "{kept:?}");
}

/// A runner that reports only what failed, over the standard one.
const QUIET: &str = "use ambient.test;
    use ambient.test.{Outcome};
    use system.test as running;
    use system.test.{Cache, Cases, Tally};

    pub fn quiet(cases: Cases, cache: Cache): Tally with running.report {
        return provide {
            [running.report](event) {
                let hidden = match event {
                    let ended: { outcome: Outcome } => test.passed(ended.outcome),
                    _ => false,
                };
                if !hidden {
                    running.report!(event);
                }
            }
        } => running.tests(cases, cache);
    }

    #[test.case] fn holds(): null => null;
    #[test.case] fn faults(): int => 1 / 0;";

#[test]
fn a_package_runs_its_tests_its_own_way() {
    let (code, said) = Runs::default().tested("[test]\nrun = \"pkg.quiet\"", QUIET);
    let lines: Vec<&str> = said.lines().filter(|line| !line.starts_with(' ')).collect();

    assert_eq!(code, 1);
    assert_eq!(lines, ["FAIL  app.faults", "1 passed, 1 failed"]);
}

#[test]
fn a_runner_reaches_the_world_its_row_names() {
    let source = "use ambient.test;
        use system.console;
        use system.test as running;
        use system.test.{Cache, Cases, Tally};

        pub fn loud(cases: Cases, cache: Cache): Tally with running.report, console.print {
            console.print!(\"starting\");
            return running.tests(cases, cache);
        }

        #[test.case] fn holds(): null => null;";
    let (code, said) = Runs::default().tested("[test]\nrun = \"pkg.loud\"", source);

    assert_eq!((code, said.lines().next()), (0, Some("starting")), "{said}");
}

/// What testing says of a package whose manifest names `runner`, which
/// `source` defines, as a runner.
fn refused(runner: &str, source: &str) -> String {
    let tests = format!("{source}\n#[test.case] fn holds(): null => null;");
    let (code, said) = Runs::default().tested(&format!("[test]\nrun = \"pkg.{runner}\""), &tests);
    assert_eq!(code, 1, "{said}");
    said.lines().next().unwrap_or_default().to_owned()
}

#[test]
fn a_runner_not_typed_as_one_is_refused_where_it_is() {
    let source = "use ambient.test; pub fn wrong(): int => 1;";

    assert_eq!(
        refused("wrong", source),
        "error: app/src/lib.ab:1:23: app:.wrong: what runs tests takes \
         `system.test.Cases` and `system.test.Cache`, and gives `system.test.Tally`"
    );
}

#[test]
fn a_runner_that_takes_what_the_target_does_not_bind_is_refused_where_it_is() {
    let source = "use ambient.test;
        use system.test as running;
        use system.test.{Cache, Cases, Tally};
        pub ability asked(): int;
        pub fn asking(cases: Cases, cache: Cache): Tally with running.report, asked {
            asked!();
            return running.tests(cases, cache);
        }";

    let said = refused("asking", source);
    assert!(said.starts_with("error: app/src/lib.ab:5:"), "{said}");
    assert!(said.contains("app:.asking"), "{said}");
}

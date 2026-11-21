//! Dependencies in git repositories, and the lock that holds the
//! commit and snapshot each led to. See "Git dependencies" and "Locks"
//! in `docs/spec/releases.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::path::PathBuf;

use ambient::cli::{Chosen, Command};
use support::packages::{Files, Package, Remote, branch, files, package, pinned, rev};

const GIT: &str = "https://example.com/numbers";
const SEVEN: &str = "pub fn seven(): int => 7;";

/// An `app` whose `main` gives what `seven` of the package it calls
/// `numbers` gives, which it pins at the commit `rev`.
fn app(rev: &str) -> Package {
    let app = package("app", "app").on("numbers", &pinned(GIT, rev));
    app.library(
        "use numbers.seven;
        pub fn main(): int => seven();",
    )
}

/// The package `numbers` as the commit `at` has it.
fn numbers(at: &'static str) -> Package {
    package(at, "n").library(SEVEN)
}

/// An `app` that pins the commit of all ones, which is at `one/`.
fn pinning() -> Files {
    files(None, [app(&rev('1')), numbers("one")])
}

fn remote(files: &Files) -> Remote<'_> {
    Remote::new(files, [(GIT, &rev('1'), "one"), (GIT, &rev('2'), "two")])
}

fn run() -> Command {
    Command::Run {
        at: Chosen::default(),
        bin: "main".to_owned(),
        args: Vec::new(),
    }
}

/// What running `app` exits with on `machine`, or what it reports.
fn ran(machine: &Remote) -> Result<i32, String> {
    let world = support::scripted::Scripted::within("app");
    let store = ambient::store::MemoryStore::new();
    let (code, world) = world.execute_from(machine.files, &store, machine, &run());
    if world.err.is_empty() {
        Ok(code)
    } else {
        Err(world.err)
    }
}

/// The files once `app` has been run on a machine that kept nothing:
/// with the lock that wrote.
fn locked(files: &Files) -> Files {
    remote(files).written(&run()).unwrap()
}

#[test]
fn a_pinned_dependency_is_fetched_and_locked() {
    let files = pinning();
    let machine = remote(&files);
    let after = machine.written(&run()).unwrap();

    assert_eq!(*machine.fetched.borrow(), [GIT]);
    let lock = &after[&PathBuf::from("app/ambient.lock")];
    let held = "id = \"n\"\nversion = \"1.0.0\"\nsnapshot = \"";
    assert!(lock.starts_with(&format!("[[snapshot]]\n{held}")), "{lock}");
    assert!(lock.ends_with(&format!("\"\ngit = \"{GIT}\"\nrev = \"{}\"\n", rev('1'))));
    assert_eq!(ran(&remote(&after)), Ok(7));
}

#[test]
fn what_is_locked_and_kept_is_not_fetched_again() {
    let files = locked(&pinning());
    let machine = remote(&files);

    assert_eq!(ran(&machine), Ok(7));
    assert_eq!(ran(&machine), Ok(7));
    // Once, since the machine kept nothing; the lock is not written
    // again either.
    assert_eq!(machine.fetched.borrow().len(), 1);
    assert_eq!(machine.written(&run()).unwrap(), files);
}

#[test]
fn what_is_fetched_must_be_what_was_locked() {
    let mut files = locked(&pinning());
    files.insert(
        "one/src/lib.ab".into(),
        "pub fn seven(): int => 8;".to_owned(),
    );

    let said = ran(&remote(&files)).unwrap_err();
    assert!(
        said.starts_with(&format!("error: {GIT} is not what was locked: ")),
        "{said}"
    );
}

#[test]
fn what_is_kept_must_be_what_was_locked() {
    // The machine fetched it as it was, and its files changed since.
    let files = locked(&pinning());
    let machine = remote(&files);
    assert_eq!(ran(&machine), Ok(7));
    let mut changed = files.clone();
    changed.insert(
        "one/src/lib.ab".into(),
        "pub fn seven(): int => 8;".to_owned(),
    );
    let kept = machine.kept.into_inner();
    let machine = Remote {
        kept: kept.into(),
        ..remote(&changed)
    };

    let said = ran(&machine).unwrap_err();
    assert!(
        said.starts_with("error: one is not what was locked: "),
        "{said}"
    );
}

#[test]
fn a_pin_that_changes_is_fetched_and_locked_in_place_of_the_old_one() {
    let two = package("two", "n").library("pub fn seven(): int => 70;");
    let mut files = locked(&files(None, [app(&rev('1')), numbers("one"), two]));
    let moved = app(&rev('2')).manifest;
    files.insert("app/ambient.toml".into(), moved);
    let after = remote(&files).written(&run()).unwrap();

    assert_eq!(ran(&remote(&after)), Ok(70));
    let lock = &after[&PathBuf::from("app/ambient.lock")];
    assert_eq!(lock.matches("[[snapshot]]").count(), 1, "{lock}");
    assert!(lock.contains(&rev('2')), "{lock}");
}

#[test]
fn a_lock_holds_what_every_member_declares_and_nothing_else() {
    // `app` pins one commit and `tool`, which it never opens, another.
    let two = package("two", "n").library(SEVEN);
    let tool = package("tool", "tool").on("numbers", &pinned(GIT, &rev('2')));
    let members = Some(&["app", "tool"][..]);
    let files = files(members, [app(&rev('1')), tool, numbers("one"), two]);
    let both = remote(&files).written(&run()).unwrap();
    let lock = |files: &Files| files[&PathBuf::from("ambient.lock")].clone();
    assert_eq!(lock(&both).matches("[[snapshot]]").count(), 2);

    // Once `tool` declares nothing, what was locked for it goes.
    let mut one = both.clone();
    one.insert("tool/ambient.toml".into(), package("tool", "tool").manifest);
    let one = remote(&one).written(&run()).unwrap();
    assert_eq!(lock(&one).matches("[[snapshot]]").count(), 1);
    assert!(lock(&one).contains(&rev('1')));
}

/// An `app` on the branch `main` of `numbers`, and `numbers` as it is
/// at two commits: one that gives 7 and one that gives 70.
fn following() -> Files {
    let app = package("app", "app").on("numbers", &branch(GIT, "main"));
    let app = app.library("use numbers.seven; pub fn main(): int => seven();");
    let two = package("two", "n").library("pub fn seven(): int => 70;");
    files(None, [app, numbers("one"), two])
}

/// A machine on which `main` is at the commit of all `digit`.
fn main_at(files: &Files, digit: char) -> Remote<'_> {
    remote(files).branch(GIT, "main", &rev(digit))
}

fn update(name: Option<&str>) -> Command {
    Command::Update {
        name: name.map(str::to_owned),
        at: Chosen::default(),
    }
}

#[test]
fn a_branch_is_read_once_and_the_commit_it_led_to_locked() {
    let files = following();
    let after = main_at(&files, '1').written(&run()).unwrap();
    let lock = &after[&PathBuf::from("app/ambient.lock")];
    let held = format!(
        "git = \"{GIT}\"\nbranch = \"main\"\nrev = \"{}\"\n",
        rev('1')
    );
    assert!(lock.ends_with(&held), "{lock}");

    // The branch has moved, and nobody asks.
    let moved = main_at(&after, '2');
    assert_eq!(ran(&moved), Ok(7));
    assert_eq!(*moved.resolved.borrow(), [""; 0]);
}

#[test]
fn an_update_reads_a_branch_again_and_locks_where_it_is() {
    let files = main_at(&following(), '1').written(&run()).unwrap();
    for name in [None, Some("numbers")] {
        let after = main_at(&files, '2').written(&update(name)).unwrap();
        let lock = &after[&PathBuf::from("app/ambient.lock")];
        assert_eq!(lock.matches("[[snapshot]]").count(), 1, "{lock}");
        assert_eq!(ran(&main_at(&after, '2')), Ok(70));
    }
    // One that has not moved leaves the lock as it is.
    assert_eq!(main_at(&files, '1').written(&update(None)), Ok(files));
}

#[test]
fn an_update_is_of_a_dependency_that_moves() {
    let files = locked(&pinning());
    for name in ["numbers", "nothing"] {
        let said = format!(
            "error: app: `{name}` is no dependency on a branch or a tag, so nothing updates it\n"
        );
        assert_eq!(remote(&files).written(&update(Some(name))), Err(said));
    }
}

#[test]
fn two_pins_of_one_package_are_two_instances() {
    let two = package("two", "n").library("pub fn seventy(): int => 70;");
    let app = app(&rev('1'))
        .on("numbers_next", &pinned(GIT, &rev('2')))
        .library(
            "use numbers.seven; use numbers_next.seventy;
            pub fn main(): int => seven() + seventy();",
        );
    let files = files(None, [app, numbers("one"), two]);

    assert_eq!(ran(&remote(&files)), Ok(77));
}

#[test]
fn a_lock_holds_the_id_the_package_in_a_repository_says() {
    // No declaration says one, so a member stands in by what is held.
    let lock = &locked(&pinning())[&PathBuf::from("app/ambient.lock")];
    assert!(lock.contains("id = \"n\""), "{lock}");
}

#[test]
fn a_dependency_that_cannot_be_fetched_is_reported() {
    let files = files(None, [app(&rev('3'))]);

    assert_eq!(
        ran(&remote(&files)),
        Err(format!(
            "error: app: the dependency `numbers` was not fetched: {GIT} has no commit {}\n",
            rev('3')
        ))
    );
}

#[test]
fn a_fetched_package_names_no_directory() {
    let numbers = numbers("one").on("other", "path = \"../other\"");
    let files = files(None, [app(&rev('1')), numbers, package("other", "o")]);

    assert_eq!(
        ran(&remote(&files)),
        Err("error: one/ambient.toml: the dependency `other` names a directory\n".to_owned())
    );
}

#[test]
fn a_fetched_package_pins_what_it_depends_on_into_the_same_lock() {
    let far = "https://example.com/far";
    let twice = package("one", "t")
        .on("n", &pinned(far, &rev('5')))
        .library("use n.seven; pub fn fourteen(): int => seven() + seven();");
    let app = package("app", "app").on("twice", &pinned(GIT, &rev('1')));
    let app = app.library("use twice.fourteen; pub fn main(): int => fourteen();");
    let files = files(None, [app, twice, numbers("five")]);
    let commits = [(GIT, &*rev('1'), "one"), (far, &*rev('5'), "five")];
    let after = Remote::new(&files, commits).written(&run()).unwrap();

    let lock = &after[&PathBuf::from("app/ambient.lock")];
    assert_eq!(lock.matches("[[snapshot]]").count(), 2, "{lock}");
    assert_eq!(ran(&Remote::new(&after, commits)), Ok(14));
}

#[test]
fn a_member_stands_in_for_a_pinned_dependency() {
    let near = package("near", "n").library("pub fn seven(): int => 700;");
    let mut files = locked(&pinning());
    files.extend(support::packages::files(Some(&["app", "near"]), [near]));
    // The lock is the workspace's, which is now the directory above.
    let lock = files.remove(&PathBuf::from("app/ambient.lock")).unwrap();
    files.insert("ambient.lock".into(), lock);

    assert_eq!(ran(&remote(&files)), Ok(700));
}

#[test]
fn a_pinned_dependency_is_vendored_and_built_with_nothing_to_fetch() {
    let files = locked(&pinning());
    let vendor = Command::Vendor {
        name: Some("numbers".to_owned()),
        at: Chosen::default(),
    };
    let mut files = remote(&files).written(&vendor).unwrap();
    // The pin still says where the copy came from.
    let manifest = &files[&PathBuf::from("app/ambient.toml")];
    assert!(manifest.contains(&pinned(GIT, &rev('1'))), "{manifest}");

    files.retain(|path, _| !path.starts_with("one"));
    let nowhere = Remote::new(&files, []);
    assert_eq!(ran(&nowhere), Ok(7));
    assert_eq!(*nowhere.fetched.borrow(), [""; 0]);
}

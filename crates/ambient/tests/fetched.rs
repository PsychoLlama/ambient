//! Fetching a dependency from a git repository into what a machine
//! keeps. See "Fetching" in `docs/spec/releases.md`.
//!
//! These tests run `git` and write under the system's temporary
//! directory, and remove what they wrote.

#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use ambient::project::{Disk, Kept, LOCK, Project, open_from};

/// A directory of this test's own, removed when it is dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!("ambient-fetch-{}-{unique}", std::process::id());
        Self(std::env::temp_dir().join(name))
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What `git` says when run in `directory`, as nobody in particular.
fn git(directory: &Path, args: &[&str]) -> String {
    let nobody = ["user.name=nobody", "user.email=nobody@example.invalid"];
    let ran = Command::new("git")
        .args(nobody.iter().flat_map(|setting| ["-c", setting]))
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(directory)
        .output()
        .unwrap();
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    String::from_utf8(ran.stdout).unwrap().trim().to_owned()
}

/// A repository in `scratch` that holds the package `numbers` in a
/// directory of its own, among files that are not the package's. Gives
/// the commit.
fn repository(scratch: &Scratch) -> String {
    let manifest = "name = \"numbers\"\nid = \"n\"\nversion = \"1.2.0\"\nfiles = [\"notes.txt\"]";
    scratch.write("repo/packages/numbers/ambient.toml", manifest);
    scratch.write(
        "repo/packages/numbers/src/lib.ab",
        "pub fn seven(): int => 7;",
    );
    scratch.write("repo/packages/numbers/notes.txt", "kept");
    scratch.write("repo/packages/numbers/draft.txt", "left behind");
    scratch.write("repo/README.md", "not the package's");
    let repo = scratch.0.join("repo");
    git(&repo, &["init", "--quiet"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "numbers"]);
    // A tag that is an object of its own, which is not the commit.
    git(&repo, &["tag", "-a", "v1", "-m", "the first"]);
    git(&repo, &["rev-parse", "HEAD"])
}

/// An `app` in `scratch` that depends on `numbers` at `state` of the
/// repository, opened on a machine that keeps snapshots there too.
fn opened(scratch: &Scratch, state: &str) -> Project {
    let repo = scratch.0.join("repo");
    let manifest = format!(
        "name = \"app\"\nid = \"app\"\nversion = \"0.1.0\"\n[dependencies]\n\
         numbers = {{ id = \"n\", git = \"{}\", {state} directory = \"packages/numbers\" }}",
        repo.display()
    );
    scratch.write("app/ambient.toml", &manifest);
    scratch.write(
        "app/src/main.ab",
        "use numbers.seven; pub fn main(): int => seven();",
    );
    let kept = Kept::under(scratch.0.join("kept"));
    let project = open_from(&Disk, &kept, &scratch.0.join("app")).unwrap();
    if let Some((lock, text)) = &project.locked {
        std::fs::write(lock, text).unwrap();
    }
    project
}

/// The names of what `directory` holds, in order.
fn names(directory: &Path) -> Vec<String> {
    let name = |entry: std::io::Result<std::fs::DirEntry>| {
        entry.unwrap().file_name().to_string_lossy().into_owned()
    };
    let mut names: Vec<String> = std::fs::read_dir(directory).unwrap().map(name).collect();
    names.sort();
    names
}

#[test]
fn a_commit_is_fetched_and_only_the_package_is_kept() {
    let scratch = Scratch::new();
    let rev = repository(&scratch);
    let project = opened(&scratch, &format!("rev = \"{rev}\","));

    let lock = std::fs::read_to_string(scratch.0.join("app").join(LOCK)).unwrap();
    assert!(lock.contains(&format!("rev = \"{rev}\"")), "{lock}");
    let numbers = project
        .packages
        .iter()
        .find(|(name, _)| name.starts_with("numbers@"));
    let kept = &numbers.unwrap().1.directory;
    assert_eq!(names(kept), ["ambient.toml", "notes.txt", "src"]);
    // Nothing of the fetch is left beside what is kept.
    assert_eq!(names(&scratch.0.join("kept")).len(), 1);
}

#[test]
fn what_is_kept_is_opened_without_the_repository() {
    let scratch = Scratch::new();
    repository(&scratch);
    opened(&scratch, "tag = \"v1\",");
    std::fs::remove_dir_all(scratch.0.join("repo")).unwrap();

    let again = opened(&scratch, "tag = \"v1\",");
    assert_eq!(again.locked, None);
}

#[test]
fn a_tag_and_a_repository_s_own_branch_lead_to_a_commit() {
    for state in ["tag = \"v1\",", ""] {
        let scratch = Scratch::new();
        let rev = repository(&scratch);
        opened(&scratch, state);

        let lock = std::fs::read_to_string(scratch.0.join("app").join(LOCK)).unwrap();
        assert!(
            lock.contains(&format!("rev = \"{rev}\"")),
            "{state}: {lock}"
        );
    }
}

#[test]
fn a_branch_a_repository_lacks_is_reported() {
    let scratch = Scratch::new();
    repository(&scratch);
    scratch.write(
        "app/ambient.toml",
        &format!(
            "name = \"app\"\nid = \"app\"\nversion = \"0.1.0\"\n[dependencies]\n\
             numbers = {{ id = \"n\", git = \"{}\", branch = \"absent\" }}",
            scratch.0.join("repo").display()
        ),
    );
    let kept = Kept::under(scratch.0.join("kept"));
    let said = open_from(&Disk, &kept, &scratch.0.join("app")).unwrap_err();
    assert!(
        said.to_string().ends_with("repo has no branch `absent`"),
        "{said}"
    );
}

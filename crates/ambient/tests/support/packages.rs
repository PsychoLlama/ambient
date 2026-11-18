//! Packages written out as files held in memory, for tests of how
//! they find each other.

use std::collections::BTreeMap;
use std::path::PathBuf;

use std::cell::RefCell;

use ambient::cli::Command;
use ambient::hash::Hash;
use ambient::project::{Contents, Manifest, Pin, Snapshots};
use ambient::store::MemoryStore;

use super::scripted::Scripted;

pub type Files = BTreeMap<PathBuf, String>;

/// A package to write out: where it is, and what its manifest says.
pub struct Package {
    at: &'static str,
    pub manifest: String,
    library: &'static str,
}

/// A package at `at`, named for it, with the ID `id` at version 1.0.0.
pub fn package(at: &'static str, id: &str) -> Package {
    Package::versioned(at, id, "1.0.0")
}

impl Package {
    pub fn versioned(at: &'static str, id: &str, version: &str) -> Self {
        let name = at.rsplit('/').next().unwrap();
        let manifest =
            format!("name = \"{name}\"\nid = \"{id}\"\nversion = \"{version}\"\n[dependencies]\n");
        Self {
            at,
            manifest,
            library: "",
        }
    }

    /// Depends on the package `id` as `name`, `found` saying where it
    /// is or which version: `path = ".."` or `version = ".."`.
    pub fn on(mut self, name: &str, id: &str, found: &str) -> Self {
        let dependency = format!("{name} = {{ id = \"{id}\", {found} }}\n");
        self.manifest.push_str(&dependency);
        self
    }

    /// Has `source` for its library module.
    pub fn library(mut self, source: &'static str) -> Self {
        self.library = source;
        self
    }
}

/// The files of `packages`, of which one named `app` says its `main`
/// in the module `main`, under a workspace at the top that lists
/// `members`.
pub fn files<const N: usize>(members: Option<&[&str]>, packages: [Package; N]) -> Files {
    let mut files = Files::new();
    if let Some(members) = members {
        files.insert("ambient.toml".into(), workspace(members));
    }
    for package in packages {
        let at = PathBuf::from(package.at);
        let module = if at.ends_with("app") { "main" } else { "lib" };
        files.insert(at.join("ambient.toml"), package.manifest);
        files.insert(
            at.join(format!("src/{module}.ab")),
            package.library.to_owned(),
        );
    }
    files
}

/// The manifest of a workspace that lists `members`.
pub fn workspace(members: &[&str]) -> String {
    let listed: Vec<String> = members.iter().map(|at| format!("\"{at}\"")).collect();
    format!("members = [{}]", listed.join(", "))
}

/// What running `app` exits with, or the error it reports.
pub fn ran(files: &Files) -> Result<i32, String> {
    ran_at(files, "app")
}

/// What running the package at `at` exits with, or the error it
/// reports.
pub fn ran_at(files: &Files, at: &str) -> Result<i32, String> {
    let command = Command::Run {
        path: PathBuf::from(at),
        entry: "main.main".to_owned(),
        args: Vec::new(),
    };
    let (code, world) = Scripted::default().execute(files, &command);
    if world.err.is_empty() {
        Ok(code)
    } else {
        Err(world.err)
    }
}

/// `files` with what `command` wrote over them, or what it refused
/// with.
pub fn written(mut files: Files, command: &Command) -> Result<Files, String> {
    let (_, world) = Scripted::default().execute(&files, command);
    if !world.err.is_empty() {
        return Err(world.err);
    }
    for (path, bytes) in world.files {
        let text = String::from_utf8(bytes).unwrap();
        files.insert(PathBuf::from(path), text);
    }
    Ok(files)
}

/// A commit, written out whole, that is all the digit `digit`.
pub fn rev(digit: char) -> String {
    digit.to_string().repeat(40)
}

/// How a dependency pins the commit `rev` of the repository `git`.
pub fn pinned(git: &str, rev: &str) -> String {
    format!("git = \"{git}\", rev = \"{rev}\"")
}

/// Repositories held in the same files as everything else, and a
/// machine that keeps what it fetched from them.
pub struct Remote<'a> {
    pub files: &'a Files,
    /// Where the files of each commit are, by repository and commit.
    pub commits: BTreeMap<(String, String), PathBuf>,
    /// What the machine keeps: where each snapshot was fetched to.
    pub kept: RefCell<BTreeMap<Hash, PathBuf>>,
    /// Every repository that was fetched from, in order.
    pub fetched: RefCell<Vec<String>>,
}

impl<'a> Remote<'a> {
    /// A machine that keeps nothing yet, and can fetch each of
    /// `commits`: a repository, a commit, and where its files are.
    pub fn new<const N: usize>(files: &'a Files, commits: [(&str, &str, &str); N]) -> Self {
        let commit = |(git, rev, at): (&str, &str, &str)| {
            ((git.to_owned(), rev.to_owned()), PathBuf::from(at))
        };
        Self {
            files,
            commits: commits.into_iter().map(commit).collect(),
            kept: RefCell::default(),
            fetched: RefCell::default(),
        }
    }

    /// What `command` over the files comes to on this machine, with
    /// what it wrote: the files after it, or what it refused with.
    pub fn written(&self, command: &Command) -> Result<Files, String> {
        let world = Scripted::default();
        let store = MemoryStore::new();
        let (_, world) = world.execute_from(self.files, &store, self, command);
        if !world.err.is_empty() {
            return Err(world.err);
        }
        let mut files = self.files.clone();
        for (path, bytes) in world.files {
            files.insert(PathBuf::from(path), String::from_utf8(bytes).unwrap());
        }
        Ok(files)
    }
}

impl Snapshots for Remote<'_> {
    fn held(&self, snapshot: &Hash) -> Option<PathBuf> {
        self.kept.borrow().get(snapshot).cloned()
    }

    fn fetch(&self, pin: &Pin) -> Result<Hash, String> {
        self.fetched.borrow_mut().push(pin.git.clone());
        let commit = (pin.git.clone(), pin.rev.clone());
        let missing = || format!("{} has no commit {}", pin.git, pin.rev);
        let directory = self.commits.get(&commit).ok_or_else(missing)?;
        let directory: PathBuf = directory.iter().chain(&pin.directory).collect();
        let manifest = &self.files[&directory.join("ambient.toml")];
        let contents = Contents {
            directory: directory.clone(),
            listed: Manifest::read(manifest)?.package.unwrap().files,
        };
        let snapshot = contents.snapshot(self.files).unwrap();
        self.kept.borrow_mut().insert(snapshot, directory);
        Ok(snapshot)
    }
}

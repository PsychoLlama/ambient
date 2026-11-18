//! A workspace's lock: each snapshot it builds that is not a member,
//! and where it was fetched from. See "Locks" and "Git
//! dependencies" in `spec/releases.md`.

use std::path::{Path, PathBuf};

use ambient_host::hash::Hash;
use serde::{Deserialize, Serialize};

use super::version::Version;

/// The name of a workspace's lock, beside its manifest.
pub const LOCK: &str = "ambient.lock";

/// A package in a git repository: where it is, and which state of the
/// repository is meant. A declaration says a branch, a tag, a commit,
/// or none of them, which is the repository's own branch; what is
/// locked for one says the same and the commit that led to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Source {
    /// The repository.
    pub git: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// The commit, written out whole.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// The package's directory in the repository.
    #[serde(default, skip_serializing_if = "top")]
    pub directory: PathBuf,
}

impl Source {
    /// Whether what the source means may change: it names no commit.
    #[must_use]
    pub fn moves(&self) -> bool {
        self.rev.is_none()
    }

    /// The source, at the commit `rev`.
    #[must_use]
    pub fn at(&self, rev: String) -> Self {
        let rev = Some(rev);
        Self {
            rev,
            ..self.clone()
        }
    }
}

/// One commit of a repository, and a package's directory in it.
#[derive(Clone, Copy, Debug)]
pub struct Commit<'a> {
    pub git: &'a str,
    pub rev: &'a str,
    pub directory: &'a Path,
}

/// Whether a directory in a repository is its top, which goes unsaid.
#[allow(clippy::ptr_arg, reason = "serde hands a field as it is declared")]
fn top(directory: &PathBuf) -> bool {
    directory.as_os_str().is_empty()
}

/// A hash as a lock writes one: the text anything else shows it as.
mod written {
    use ambient_host::hash::Hash;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(hash: &Hash, to: S) -> Result<S::Ok, S::Error> {
        to.collect_str(hash)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<Hash, D::Error> {
        let text = String::deserialize(from)?;
        Hash::parse(&text).ok_or_else(|| D::Error::custom(format!("`{text}` is no snapshot")))
    }
}

/// A snapshot a workspace builds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Locked {
    /// The ID of the package it is a snapshot of.
    pub id: String,
    pub version: Version,
    #[serde(with = "written")]
    pub snapshot: Hash,
    /// What was declared, and the commit it led to.
    #[serde(flatten)]
    pub source: Source,
}

impl Locked {
    /// The commit the snapshot is at.
    #[must_use]
    pub fn commit(&self) -> Commit<'_> {
        Commit {
            git: &self.source.git,
            rev: self.source.rev.as_deref().unwrap_or_default(),
            directory: &self.source.directory,
        }
    }

    /// Whether this is what a declaration of the package `id` at
    /// `source` leads to: the same, at whatever commit if it says none.
    fn satisfies(&self, id: &str, source: &Source) -> bool {
        let commit = source.rev.as_ref().or(self.source.rev.as_ref());
        self.id == id && self.source == source.at(commit.cloned().unwrap_or_default())
    }
}

/// A lock, as it is written.
#[derive(Default, Serialize, Deserialize)]
struct Written {
    #[serde(default)]
    snapshot: Vec<Locked>,
}

/// What a workspace has locked, in the order it is written in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lock {
    snapshots: Vec<Locked>,
}

/// What a lock is in the order of: one that does not depend on when
/// each entry was locked, so that changes made apart merge.
fn order(one: &Locked, other: &Locked) -> std::cmp::Ordering {
    (&one.id, &one.source).cmp(&(&other.id, &other.source))
}

impl Lock {
    /// Reads a lock's text.
    ///
    /// # Errors
    ///
    /// The text is not a lock, and why.
    pub fn read(text: &str) -> Result<Self, String> {
        let written: Written = toml::from_str(text).map_err(|error| error.message().to_owned())?;
        let mut snapshots = written.snapshot;
        if snapshots.iter().any(|locked| locked.source.moves()) {
            return Err("a snapshot says no `rev`".to_owned());
        }
        snapshots.sort_by(order);
        Ok(Self { snapshots })
    }

    /// The lock as text, one entry to a snapshot.
    #[must_use]
    pub fn written(&self) -> String {
        let snapshot = self.snapshots.clone();
        toml::to_string(&Written { snapshot }).unwrap_or_default()
    }

    /// What is locked for a declaration of the package `id` at
    /// `source`.
    #[must_use]
    pub fn find(&self, id: &str, source: &Source) -> Option<&Locked> {
        self.snapshots
            .iter()
            .find(|locked| locked.satisfies(id, source))
    }

    /// Locks `locked`, if it is not locked already.
    pub fn insert(&mut self, locked: Locked) {
        if let Err(place) = self.snapshots.binary_search_by(|held| order(held, &locked)) {
            self.snapshots.insert(place, locked);
        }
    }
}

/// The snapshots a machine keeps, and how it comes by one it lacks.
/// See "Fetching" in `spec/releases.md`.
pub trait Snapshots {
    /// Where the files of a snapshot are, if the machine keeps it.
    fn held(&self, snapshot: &Hash) -> Option<PathBuf>;

    /// The commit a source that names none is at now.
    ///
    /// # Errors
    ///
    /// The repository was not reached, or has no such branch or tag.
    fn resolve(&self, source: &Source) -> Result<String, String>;

    /// Fetches the package at `commit`, keeps it, and gives its
    /// snapshot.
    ///
    /// # Errors
    ///
    /// It could not be fetched, or what was fetched is no package.
    fn fetch(&self, commit: Commit) -> Result<Hash, String>;

    /// The bytes at a URL, for a build that names their digest. See
    /// "Build abilities" in `spec/tools.md`.
    ///
    /// # Errors
    ///
    /// Nothing answered there, or this machine fetches nothing.
    fn bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        Err(format!("nothing here fetches {url}"))
    }
}

/// A machine that keeps nothing and fetches nothing.
pub struct Nowhere;

impl Snapshots for Nowhere {
    fn held(&self, _: &Hash) -> Option<PathBuf> {
        None
    }

    fn resolve(&self, source: &Source) -> Result<String, String> {
        Err(format!("nothing reaches {} here", source.git))
    }

    fn fetch(&self, commit: Commit) -> Result<Hash, String> {
        Err(format!("nothing fetches {} here", commit.git))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ambient_host::hash::Hasher;

    use super::{Lock, Locked, Source};
    use crate::project::Version;

    fn source(branch: Option<&str>, rev: Option<&str>) -> Source {
        Source {
            git: "https://example.com/repo".to_owned(),
            branch: branch.map(str::to_owned),
            tag: None,
            rev: rev.map(str::to_owned),
            directory: PathBuf::new(),
        }
    }

    /// The package `id` locked at the commit `rev` its branch led to.
    fn locked(id: &str, branch: Option<&str>, rev: &str) -> Locked {
        Locked {
            id: id.to_owned(),
            version: Version::new(1, 2, 0),
            snapshot: Hasher::new(rev).finish(),
            source: source(branch, Some(rev)),
        }
    }

    fn lock<const N: usize>(entries: [Locked; N]) -> Lock {
        let mut lock = Lock::default();
        for locked in entries {
            lock.insert(locked);
        }
        lock
    }

    #[test]
    fn a_lock_is_read_as_it_was_written() {
        let lock = lock([locked("b", Some("main"), "22"), locked("a", None, "11")]);
        assert_eq!(Lock::read(&lock.written()), Ok(lock));
    }

    #[test]
    fn a_lock_is_the_same_whatever_was_locked_first() {
        let (one, other) = (locked("a", None, "11"), locked("b", None, "22"));
        assert_eq!(lock([one.clone(), other.clone()]), lock([other, one]));
    }

    #[test]
    fn a_declaration_finds_what_was_locked_for_it() {
        let lock = lock([locked("a", Some("main"), "11")]);
        let found = |branch, rev| lock.find("a", &source(branch, rev)).is_some();
        // A branch is locked at whatever commit it led to.
        assert!(found(Some("main"), None));
        assert!(!found(Some("next"), None));
        assert!(!found(None, None));
        // A commit is locked only as itself.
        assert!(!found(None, Some("11")));
        assert!(lock.find("b", &source(Some("main"), None)).is_none());
    }

    #[test]
    fn an_entry_that_says_no_commit_is_no_lock() {
        let text = lock([locked("a", None, "11")]).written();
        let without = text.replace("rev = \"11\"\n", "");
        assert_eq!(
            Lock::read(&without),
            Err("a snapshot says no `rev`".to_owned())
        );
    }
}

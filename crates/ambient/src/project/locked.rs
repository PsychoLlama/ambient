//! A workspace's lock: each snapshot it builds that is not a member,
//! and where it was fetched from. See "Locks" and "Pinned
//! dependencies" in `spec/releases.md`.

use std::path::PathBuf;

use ambient_host::hash::Hash;
use serde::{Deserialize, Serialize};

use super::manifest::Version;

/// The name of a workspace's lock, beside its manifest.
pub const LOCK: &str = "ambient.lock";

/// Exactly which state of a package a dependency pins, and where it
/// is.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Pin {
    /// The repository.
    pub git: String,
    /// The commit, written out whole.
    pub rev: String,
    /// The package's directory in the repository.
    #[serde(default, skip_serializing_if = "top")]
    pub directory: PathBuf,
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
    #[serde(flatten)]
    pub pin: Pin,
}

/// A lock, as it is written.
#[derive(Default, Serialize, Deserialize)]
struct Written {
    #[serde(default)]
    snapshot: Vec<Locked>,
}

/// What a workspace has locked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lock {
    snapshots: Vec<Locked>,
}

impl Lock {
    /// Reads a lock's text.
    ///
    /// # Errors
    ///
    /// The text is not a lock, and why.
    pub fn read(text: &str) -> Result<Self, String> {
        let written: Written = toml::from_str(text).map_err(|error| error.message().to_owned())?;
        Ok(Self {
            snapshots: written.snapshot,
        })
    }

    /// The lock as text: one entry to a snapshot, in an order that
    /// does not depend on when each was locked, so that changes made
    /// apart merge.
    #[must_use]
    pub fn written(&self) -> String {
        let mut snapshot = self.snapshots.clone();
        snapshot.sort_by(|one, other| (&one.id, &one.pin).cmp(&(&other.id, &other.pin)));
        toml::to_string(&Written { snapshot }).unwrap_or_default()
    }

    /// What is locked for a dependency on the package `id` pinned at
    /// `pin`.
    #[must_use]
    pub fn find(&self, id: &str, pin: &Pin) -> Option<&Locked> {
        let pinned = |locked: &&Locked| locked.id == id && locked.pin == *pin;
        self.snapshots.iter().find(pinned)
    }

    pub fn insert(&mut self, locked: Locked) {
        self.snapshots.push(locked);
    }
}

/// The snapshots a machine keeps, and how it comes by one it lacks.
/// See "Fetching" in `spec/releases.md`.
pub trait Snapshots {
    /// Where the files of a snapshot are, if the machine keeps it.
    fn held(&self, snapshot: &Hash) -> Option<PathBuf>;

    /// Fetches the package `pin` names, keeps it, and gives its
    /// snapshot.
    ///
    /// # Errors
    ///
    /// It could not be fetched, or what was fetched is no package.
    fn fetch(&self, pin: &Pin) -> Result<Hash, String>;
}

/// A machine that keeps nothing and fetches nothing.
pub struct Nowhere;

impl Snapshots for Nowhere {
    fn held(&self, _: &Hash) -> Option<PathBuf> {
        None
    }

    fn fetch(&self, pin: &Pin) -> Result<Hash, String> {
        Err(format!("nothing fetches {} here", pin.git))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ambient_host::hash::Hasher;

    use super::{Lock, Locked, Pin};
    use crate::project::Version;

    fn locked(id: &str, rev: &str, directory: &str) -> Locked {
        let version = Version::try_from("1.2.0".to_owned()).unwrap_or_else(|_| unreachable!());
        Locked {
            id: id.to_owned(),
            version,
            snapshot: Hasher::new(rev).finish(),
            pin: Pin {
                git: "https://example.com/repo".to_owned(),
                rev: rev.to_owned(),
                directory: PathBuf::from(directory),
            },
        }
    }

    #[test]
    fn a_lock_is_read_as_it_was_written() {
        let mut lock = Lock::default();
        lock.insert(locked("b", "22", "packages/b"));
        lock.insert(locked("a", "11", ""));

        assert_eq!(
            Lock::read(&lock.written()).map(|read| read.written()),
            Ok(lock.written())
        );
        assert!(lock.find("a", &locked("a", "11", "").pin).is_some());
        assert!(lock.find("a", &locked("a", "12", "").pin).is_none());
    }

    #[test]
    fn a_lock_is_written_the_same_whatever_was_locked_first() {
        let (one, other) = (locked("a", "11", ""), locked("b", "22", ""));
        let mut forward = Lock::default();
        let mut backward = Lock::default();
        forward.insert(one.clone());
        forward.insert(other.clone());
        backward.insert(other);
        backward.insert(one);

        assert_eq!(forward.written(), backward.written());
    }
}

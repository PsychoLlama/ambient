//! Opening a pinned dependency: the snapshot the lock holds for it,
//! fetched if the machine does not keep it. See "Pinned dependencies"
//! and "Locks" in `spec/releases.md`.

use std::path::{Path, PathBuf};

use ambient_host::hash::Hash;

use super::members::manifest;
use super::{Contents, Dependency, Locked, Opening, Package, Pin, ProjectError};

/// A dependency that pins, and the package that declares it.
pub(super) struct Pinned<'a> {
    pub package: &'a str,
    pub name: &'a str,
    pub dependency: &'a Dependency,
    pub pin: &'a Pin,
}

impl Pinned<'_> {
    fn refuse(&self, reason: String) -> ProjectError {
        ProjectError::Fetch {
            package: self.package.to_owned(),
            name: self.name.to_owned(),
            reason,
        }
    }

    fn lost(&self) -> ProjectError {
        self.refuse("it was fetched and is not kept".to_owned())
    }
}

impl Opening<'_> {
    /// What the package at `directory` says of itself.
    fn said(&self, pinned: &Pinned, directory: &Path) -> Result<Package, ProjectError> {
        let said = manifest(self.files, directory)?.package;
        said.ok_or_else(|| pinned.refuse("it is no package".to_owned()))
    }

    /// What the lock holds for a pinned dependency, which is fetched
    /// and locked if the lock holds nothing.
    fn locked(&mut self, pinned: &Pinned) -> Result<Locked, ProjectError> {
        let Pinned {
            dependency, pin, ..
        } = pinned;
        if let Some(locked) = self.lock.find(&dependency.id, pin) {
            return Ok(locked.clone());
        }
        let snapshot = self.snapshots.fetch(pin);
        let snapshot = snapshot.map_err(|reason| pinned.refuse(reason))?;
        let kept = self.snapshots.held(&snapshot);
        let said = self.said(pinned, &kept.ok_or_else(|| pinned.lost())?)?;
        if said.id != dependency.id {
            return Err(ProjectError::Mistaken {
                package: pinned.package.to_owned(),
                name: pinned.name.to_owned(),
                path: PathBuf::from(&pin.git),
                declared: format!("package `{}`", dependency.id),
                found: format!("`{}`", said.id),
            });
        }
        let locked = Locked {
            id: said.id,
            version: said.version,
            snapshot,
            pin: (*pin).clone(),
        };
        self.lock.insert(locked.clone());
        Ok(locked)
    }

    /// Where the machine keeps the snapshot that is locked, once it
    /// does: what is fetched for it must be it.
    fn kept(&self, pinned: &Pinned, locked: &Locked) -> Result<PathBuf, ProjectError> {
        if let Some(kept) = self.snapshots.held(&locked.snapshot) {
            return Ok(kept);
        }
        let fetched = self.snapshots.fetch(pinned.pin);
        let fetched = fetched.map_err(|reason| pinned.refuse(reason))?;
        if fetched != locked.snapshot {
            return Err(altered(pinned.pin.git.clone(), locked, fetched));
        }
        self.snapshots.held(&fetched).ok_or_else(|| pinned.lost())
    }

    /// Opens the package a dependency pins: the snapshot the lock
    /// holds for it, or the member that stands in for that.
    pub(super) fn pinned(&mut self, pinned: &Pinned) -> Result<String, ProjectError> {
        let locked = self.locked(pinned)?;
        let standing = self.members.standing(&locked.id, locked.version.major);
        if let Some(directory) = standing.map(|member| member.directory.clone()) {
            return self.member(&directory);
        }
        let kept = self.kept(pinned, &locked)?;
        // What is kept is read as anything fetched is: by its hash.
        let said = self.said(pinned, &kept)?;
        let contents = Contents {
            directory: kept.clone(),
            listed: said.files.clone(),
        };
        let found = contents.snapshot(self.files);
        let found = found.map_err(|error| pinned.refuse(error.to_string()))?;
        if found != locked.snapshot {
            return Err(altered(kept.display().to_string(), &locked, found));
        }
        self.package(kept, &said, Some(locked.snapshot))
    }
}

fn altered(from: String, locked: &Locked, found: Hash) -> ProjectError {
    ProjectError::Altered {
        from,
        locked: locked.snapshot.to_string(),
        found: found.to_string(),
    }
}

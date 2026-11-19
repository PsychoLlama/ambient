//! Opening a dependency in a git repository: the snapshot the lock holds for it,
//! which the machine keeps once the lock is settled
//! (`project/settle.rs`). See "Git dependencies" and "Locks" in
//! `docs/spec/releases.md`.

use ambient_host::hash::Hash;

use super::members::manifest;
use super::{Contents, Dependency, Line, Locked, Opening, ProjectError, Source};

/// A dependency in a git repository, and the package that declares
/// it.
pub(super) struct Git<'a> {
    pub package: &'a str,
    pub name: &'a str,
    pub dependency: &'a Dependency,
    pub source: &'a Source,
}

impl Git<'_> {
    pub(super) fn refuse(&self, reason: String) -> ProjectError {
        ProjectError::Fetch {
            package: self.package.to_owned(),
            name: self.name.to_owned(),
            reason,
        }
    }

    pub(super) fn lost(&self) -> ProjectError {
        self.refuse("it was fetched and is not kept".to_owned())
    }
}

impl Opening<'_> {
    /// Opens the package a dependency in a repository means: the
    /// snapshot the lock holds for it, or the member that stands in
    /// for that.
    pub(super) fn git(&mut self, git: &Git) -> Result<String, ProjectError> {
        let locked = self.lock.find(&git.dependency.id, git.source);
        let locked = locked.cloned().ok_or_else(|| git.lost())?;
        let standing = self.members.standing(&locked.id, Line::of(&locked.version));
        if let Some(directory) = standing.map(|member| member.directory.clone()) {
            return self.member(&directory);
        }
        let kept = self.snapshots.held(&locked.snapshot);
        let kept = kept.ok_or_else(|| git.lost())?;
        // What is kept is read as anything fetched is: by its hash.
        let said = manifest(self.files, &kept)?.package;
        let said = said.ok_or_else(|| git.refuse("it is no package".to_owned()))?;
        let contents = Contents {
            directory: kept.clone(),
            listed: said.files.clone(),
        };
        let found = contents.snapshot(self.files);
        let found = found.map_err(|error| git.refuse(error.to_string()))?;
        if found != locked.snapshot {
            return Err(altered(kept.display().to_string(), &locked, found));
        }
        self.package(kept, &said, Some(locked.snapshot))
    }
}

pub(super) fn altered(from: String, locked: &Locked, found: Hash) -> ProjectError {
    ProjectError::Altered {
        from,
        locked: locked.snapshot.to_string(),
        found: found.to_string(),
    }
}

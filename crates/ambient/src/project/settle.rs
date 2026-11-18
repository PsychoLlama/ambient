//! Settling a workspace's lock against what its members declare, before
//! anything is opened: what is declared and not locked is fetched and
//! locked, and what is locked and not declared is dropped. See "Locks"
//! in `spec/releases.md`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use ambient_host::hash::Hash;

use super::members::{Members, manifest};
use super::pinned::{Pinned, altered};
use super::{Files, Line, Lock, Locked, Package, ProjectError, Snapshots};

/// A package whose declarations are yet to be read.
struct Declaring {
    /// What it is called in messages.
    name: String,
    directory: PathBuf,
    said: Package,
    /// Whether it is a snapshot the machine keeps, and so has nothing
    /// beside it.
    fetched: bool,
}

struct Settling<'a> {
    files: &'a dyn Files,
    snapshots: &'a dyn Snapshots,
    members: Members,
    /// What the workspace had locked.
    held: &'a Lock,
    /// What it locks from now on.
    lock: Lock,
    /// The snapshots whose declarations were read.
    read: BTreeSet<Hash>,
    pending: Vec<Declaring>,
}

/// The members of a workspace, with the ones its declarations lead to
/// at a directory, and its lock once it holds exactly what they
/// declare.
///
/// # Errors
///
/// A declaration leads to nothing that can be fetched, or to something
/// other than what was locked for it.
pub(super) fn settled(
    files: &dyn Files,
    snapshots: &dyn Snapshots,
    members: Members,
    held: &Lock,
) -> Result<(Members, Lock), ProjectError> {
    let pending = members.all().map(Declaring::member).collect();
    let mut settling = Settling {
        files,
        snapshots,
        members,
        held,
        lock: Lock::default(),
        read: BTreeSet::new(),
        pending,
    };
    while let Some(declaring) = settling.pending.pop() {
        settling.read(&declaring)?;
    }
    Ok((settling.members, settling.lock))
}

impl Declaring {
    fn member(member: &super::members::Member) -> Self {
        Self {
            name: member.package.name.clone(),
            directory: member.directory.clone(),
            said: member.package.clone(),
            fetched: false,
        }
    }
}

impl Settling<'_> {
    /// Reads what one package declares: each package at a directory
    /// is a member, and each that is pinned is locked.
    fn read(&mut self, declaring: &Declaring) -> Result<(), ProjectError> {
        for (name, dependency) in &declaring.said.dependencies {
            if let Some(pin) = dependency.pin() {
                self.pinned(&Pinned {
                    package: &declaring.name,
                    name,
                    dependency,
                    pin: &pin,
                })?;
            } else if let Some(path) = dependency.path.as_ref().filter(|_| !declaring.fetched) {
                let path = super::normal(&declaring.directory.join(path));
                if !self.members.holds(self.files, &path) {
                    let member = self.members.at(self.files, &path)?;
                    self.pending.push(Declaring::member(member));
                }
            }
        }
        Ok(())
    }

    /// Locks what a pinned dependency leads to, and has its
    /// declarations read in turn, unless a member stands in for it.
    fn pinned(&mut self, pinned: &Pinned) -> Result<(), ProjectError> {
        let locked = self.locked(pinned)?;
        self.lock.insert(locked.clone());
        let line = Line::of(&locked.version);
        if self.members.standing(&locked.id, line).is_some() || !self.read.insert(locked.snapshot) {
            return Ok(());
        }
        let directory = self.kept(pinned, &locked)?;
        self.pending.push(Declaring {
            name: format!("{}@{}", locked.id, &locked.snapshot.to_string()[..8]),
            said: self.said(pinned, &directory)?,
            directory,
            fetched: true,
        });
        Ok(())
    }

    /// What the package at `directory` says of itself.
    fn said(&self, pinned: &Pinned, directory: &std::path::Path) -> Result<Package, ProjectError> {
        let said = manifest(self.files, directory)?.package;
        said.ok_or_else(|| pinned.refuse("it is no package".to_owned()))
    }

    /// What is locked for a pinned dependency: what was, or else what
    /// fetching it finds.
    fn locked(&self, pinned: &Pinned) -> Result<Locked, ProjectError> {
        let Pinned {
            dependency, pin, ..
        } = pinned;
        let found = |lock: &Lock| lock.find(&dependency.id, pin).cloned();
        if let Some(locked) = found(&self.lock).or_else(|| found(self.held)) {
            return Ok(locked);
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
        Ok(Locked {
            id: said.id,
            version: said.version,
            snapshot,
            pin: (*pin).clone(),
        })
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
}

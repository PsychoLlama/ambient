//! Settling a workspace's lock against what its members declare, before
//! anything is opened: what is declared and not locked is fetched and
//! locked, and what is locked and not declared is dropped. See "Locks"
//! in `spec/releases.md`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ambient_host::hash::Hash;

use super::git::{Git, altered};
use super::members::{Member, Members, manifest};
use super::{Commit, Files, LOCK, Line, Lock, Locked, Package, ProjectError, Snapshots, Source};

/// Which dependencies on a branch or a tag are read again, whatever
/// is locked for them. See "Git dependencies" in `spec/releases.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    Nothing,
    /// Each one any member declares.
    Every,
    /// The one the package that is opened declares under this name.
    One(String),
}

/// A workspace whose lock holds exactly what its members declare.
pub(super) struct Settled {
    /// The members, with those a declaration leads to at a directory.
    pub members: Members,
    pub lock: Lock,
    /// The lock's file and what it is to say, if it does not say it.
    pub written: Option<(PathBuf, String)>,
}

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
    stale: Stale,
    /// The snapshots whose declarations were read.
    read: BTreeSet<Hash>,
    pending: Vec<Declaring>,
}

/// What is read again though it is locked.
enum Stale {
    Nothing,
    /// Every source that moves.
    Every,
    /// The one source of a package, by its ID.
    One(String, Source),
}

/// What `update` reads again, for the package at `directory`.
fn stale(
    members: &mut Members,
    files: &dyn Files,
    directory: &Path,
    update: &Update,
) -> Result<Stale, ProjectError> {
    let name = match update {
        Update::Nothing => return Ok(Stale::Nothing),
        Update::Every => return Ok(Stale::Every),
        Update::One(name) => name,
    };
    let package = &members.at(files, directory)?.package;
    let declared = package.dependencies.get(name);
    let moving =
        declared.and_then(|dependency| Some((dependency.id.clone(), dependency.source()?)));
    match moving.filter(|(_, source)| source.moves()) {
        Some((id, source)) => Ok(Stale::One(id, source)),
        None => Err(ProjectError::Fixed {
            package: package.name.clone(),
            name: name.clone(),
        }),
    }
}

/// Settles the lock of the workspace the package at `directory` is
/// in: fetches and locks what its members declare and it does not
/// hold, and drops what it holds and none declares.
///
/// # Errors
///
/// A manifest or the lock is not one, or a declaration leads to
/// nothing that can be fetched, or to something other than what was
/// locked for it.
pub(super) fn settled(
    files: &dyn Files,
    snapshots: &dyn Snapshots,
    directory: &Path,
    update: &Update,
) -> Result<Settled, ProjectError> {
    let mut members = Members::around(files, directory)?;
    let path = members.root.as_deref().unwrap_or(directory).join(LOCK);
    let held = match files.read(&path) {
        Ok(text) => Lock::read(&text).map_err(|reason| ProjectError::Manifest {
            path: path.clone(),
            reason,
        })?,
        Err(_) => Lock::default(),
    };
    let stale = stale(&mut members, files, directory, update)?;
    let pending = members.all().map(Declaring::member).collect();
    let mut settling = Settling {
        files,
        snapshots,
        members,
        held: &held,
        lock: Lock::default(),
        stale,
        read: BTreeSet::new(),
        pending,
    };
    while let Some(declaring) = settling.pending.pop() {
        settling.read(&declaring)?;
    }
    let Settling { members, lock, .. } = settling;
    let written = (lock != held).then(|| (path, lock.written()));
    Ok(Settled {
        members,
        lock,
        written,
    })
}

impl Declaring {
    fn member(member: &Member) -> Self {
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
    /// is a member, and each in a repository is locked.
    fn read(&mut self, declaring: &Declaring) -> Result<(), ProjectError> {
        for (name, dependency) in &declaring.said.dependencies {
            if let Some(source) = dependency.source() {
                self.git(&Git {
                    package: &declaring.name,
                    name,
                    dependency,
                    source: &source,
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

    /// Locks what a dependency in a repository leads to, and has its
    /// declarations read in turn, unless a member stands in for it.
    fn git(&mut self, git: &Git) -> Result<(), ProjectError> {
        let locked = self.locked(git)?;
        self.lock.insert(locked.clone());
        let line = Line::of(&locked.version);
        if self.members.standing(&locked.id, line).is_some() || !self.read.insert(locked.snapshot) {
            return Ok(());
        }
        let directory = self.kept(git, &locked)?;
        self.pending.push(Declaring {
            name: format!("{}@{}", locked.id, &locked.snapshot.to_string()[..8]),
            said: self.said(git, &directory)?,
            directory,
            fetched: true,
        });
        Ok(())
    }

    /// What the package at `directory` says of itself.
    fn said(&self, git: &Git, directory: &Path) -> Result<Package, ProjectError> {
        let said = manifest(self.files, directory)?.package;
        said.ok_or_else(|| git.refuse("it is no package".to_owned()))
    }

    /// Whether what is locked for `source` is passed over.
    fn stale(&self, id: &str, source: &Source) -> bool {
        let meant = match &self.stale {
            Stale::Nothing => false,
            Stale::Every => true,
            Stale::One(one, at) => one == id && at == source,
        };
        source.moves() && meant
    }

    /// What is locked for a dependency: what was, or else the commit
    /// it leads to now and the snapshot there.
    fn locked(&self, git: &Git) -> Result<Locked, ProjectError> {
        let Git {
            dependency, source, ..
        } = git;
        let found = |lock: &Lock| lock.find(&dependency.id, source).cloned();
        let held = || found(self.held).filter(|_| !self.stale(&dependency.id, source));
        if let Some(locked) = found(&self.lock).or_else(held) {
            return Ok(locked);
        }
        let rev = match &source.rev {
            Some(rev) => rev.clone(),
            None => self
                .snapshots
                .resolve(source)
                .map_err(|reason| git.refuse(reason))?,
        };
        let commit = Commit {
            git: &source.git,
            rev: &rev,
            directory: &source.directory,
        };
        let snapshot = self.snapshots.fetch(commit);
        let snapshot = snapshot.map_err(|reason| git.refuse(reason))?;
        let kept = self.snapshots.held(&snapshot);
        let said = self.said(git, &kept.ok_or_else(|| git.lost())?)?;
        if said.id != dependency.id {
            return Err(ProjectError::Mistaken {
                package: git.package.to_owned(),
                name: git.name.to_owned(),
                path: PathBuf::from(&source.git),
                declared: format!("package `{}`", dependency.id),
                found: format!("`{}`", said.id),
            });
        }
        Ok(Locked {
            id: said.id,
            version: said.version,
            snapshot,
            source: source.at(rev),
        })
    }

    /// Where the machine keeps the snapshot that is locked, once it
    /// does: what is fetched for it must be it.
    fn kept(&self, git: &Git, locked: &Locked) -> Result<PathBuf, ProjectError> {
        if let Some(kept) = self.snapshots.held(&locked.snapshot) {
            return Ok(kept);
        }
        let fetched = self.snapshots.fetch(locked.commit());
        let fetched = fetched.map_err(|reason| git.refuse(reason))?;
        if fetched != locked.snapshot {
            return Err(altered(locked.source.git.clone(), locked, fetched));
        }
        self.snapshots.held(&fetched).ok_or_else(|| git.lost())
    }
}

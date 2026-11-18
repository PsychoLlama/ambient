//! The packages of a workspace: the ones its manifest lists, and the
//! ones at a directory a member depends on. See "Workspaces" in
//! `spec/packages.md`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use super::manifest::{Manifest, Package};
use super::{Files, MANIFEST, ProjectError};

/// A package of the workspace.
#[derive(Clone, Debug)]
pub(super) struct Member {
    /// Where it is, as it was first reached.
    pub directory: PathBuf,
    pub package: Package,
}

/// The members found so far, each by the one way its directory is
/// written, so a package reached two ways is one member.
#[derive(Default)]
pub(super) struct Members {
    found: BTreeMap<PathBuf, Member>,
}

/// `path` with `.` and `..` worked out, as far as they can be.
pub(super) fn normal(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match (component, out.components().next_back()) {
            (Component::CurDir, _) | (Component::ParentDir, Some(Component::RootDir)) => {}
            (Component::ParentDir, Some(Component::Normal(_))) => {
                out.pop();
            }
            (other, _) => out.push(other),
        }
    }
    out
}

/// The one way `path` is written, however it was reached.
fn key(files: &dyn Files, path: &Path) -> PathBuf {
    normal(&files.absolute(path))
}

fn manifest(files: &dyn Files, directory: &Path) -> Result<Manifest, ProjectError> {
    let path = directory.join(MANIFEST);
    let text = files
        .read(&path)
        .map_err(|error| ProjectError::Unreadable {
            path: path.clone(),
            reason: error.to_string(),
        })?;
    Manifest::read(&text).map_err(|reason| ProjectError::Manifest { path, reason })
}

/// The directory above `directory`, if there is one.
fn above(files: &dyn Files, directory: &Path) -> Option<PathBuf> {
    let above = normal(&directory.join(".."));
    let written = key(files, &above);
    let nowhere = written.starts_with("..") || written == key(files, directory);
    (!nowhere).then_some(above)
}

/// The workspace `directory` is in: the nearest directory, itself or
/// above it, whose manifest lists it, and the directories that lists.
fn workspace(files: &dyn Files, directory: &Path) -> Result<Option<Vec<PathBuf>>, ProjectError> {
    let wanted = key(files, directory);
    let mut at = Some(directory.to_owned());
    while let Some(root) = at {
        // A directory with no manifest is only on the way up.
        let listed = match files.read(&root.join(MANIFEST)) {
            Ok(_) => manifest(files, &root)?.members,
            Err(_) => None,
        };
        if let Some(listed) = listed {
            let mut members: Vec<PathBuf> =
                listed.iter().map(|path| normal(&root.join(path))).collect();
            let own = key(files, &root) == wanted;
            if own || members.iter().any(|member| key(files, member) == wanted) {
                // A workspace's own directory is a member if it is a
                // package, which the caller finds out.
                members.push(root);
                return Ok(Some(members));
            }
        }
        at = above(files, &root);
    }
    Ok(None)
}

impl Members {
    /// The members of the workspace the package at `directory` is in:
    /// only that package, if no workspace lists it.
    pub fn around(files: &dyn Files, directory: &Path) -> Result<Self, ProjectError> {
        let mut members = Self::default();
        for listed in workspace(files, directory)?.unwrap_or_default() {
            // The last is the workspace's own directory, which need
            // not be a package.
            let package = manifest(files, &listed)?.package;
            if package.is_some() || key(files, &listed) == key(files, directory) {
                members.at(files, &listed)?;
            }
        }
        members.at(files, directory)?;
        Ok(members)
    }

    /// The member at `directory`, which is one from now on.
    pub fn at(&mut self, files: &dyn Files, directory: &Path) -> Result<&Member, ProjectError> {
        let written = key(files, directory);
        if !self.found.contains_key(&written) {
            let path = directory.join(MANIFEST);
            let Some(package) = manifest(files, directory)?.package else {
                let reason = "this is a workspace's manifest, not a package's".to_owned();
                return Err(ProjectError::Manifest { path, reason });
            };
            if let Some(rival) = self.standing(&package.id, package.version.major) {
                return Err(ProjectError::Rivals {
                    id: package.id,
                    major: package.version.major,
                    first: rival.directory.clone(),
                    second: directory.to_owned(),
                });
            }
            let directory = directory.to_owned();
            self.found
                .insert(written.clone(), Member { directory, package });
        }
        Ok(&self.found[&written])
    }

    /// The member that stands in for a dependency on the package `id`
    /// at `major`.
    pub fn standing(&self, id: &str, major: u64) -> Option<&Member> {
        let stands =
            |member: &&Member| member.package.id == id && member.package.version.major == major;
        self.found.values().find(stands)
    }

    /// Whether two paths are of one directory.
    pub fn same(files: &dyn Files, one: &Path, other: &Path) -> bool {
        key(files, one) == key(files, other)
    }
}

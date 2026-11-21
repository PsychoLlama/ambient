//! A dependency as a manifest declares it: where a package is, or
//! which it is. See "Dependencies" in `docs/spec/packages.md`.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use super::locked::Source;
use super::version::Version;

/// A package another depends on: where it is, or which it is. Only a
/// package that is nowhere is named by its ID, which every other says
/// in its own manifest. See "Dependencies" in `docs/spec/packages.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dependency {
    /// The package at a directory, from the directory of this
    /// manifest: a member of the workspace, as it is.
    Path(PathBuf),
    /// The package in a git repository.
    Git(Source),
    /// The package of an ID, which a member of the workspace is.
    Named {
        id: String,
        /// The least version of it that will do, whose line is the one
        /// meant. See "Selection" in `docs/spec/releases.md`.
        version: Version,
    },
}

impl Dependency {
    /// The directory the package is at, if the dependency names one.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Path(path) => Some(path),
            _ => None,
        }
    }

    /// Where the package is, if it is in a git repository.
    #[must_use]
    pub fn source(&self) -> Option<&Source> {
        match self {
            Self::Git(source) => Some(source),
            _ => None,
        }
    }
}

/// A dependency as it is written: every key any kind of one takes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Declared {
    id: Option<String>,
    version: Option<Version>,
    path: Option<PathBuf>,
    git: Option<String>,
    branch: Option<String>,
    tag: Option<String>,
    /// A commit, written out whole.
    rev: Option<String>,
    /// The package's directory in the repository, if not its top.
    directory: Option<PathBuf>,
}

/// Whether a path names something under where it starts.
pub(super) fn inside(path: &Path) -> bool {
    let part = |part| matches!(part, Component::Normal(_));
    !path.as_os_str().is_empty() && path.components().all(part)
}

impl Declared {
    /// The dependency this says, or what is wrong with how it says
    /// where its package is.
    pub(super) fn read(self) -> Result<Dependency, &'static str> {
        let states = [&self.branch, &self.tag, &self.rev];
        let states = states.into_iter().flatten().count();
        match (self.git, self.path) {
            (Some(_), Some(_)) => Err("says both a `path` and a `git`"),
            // Nothing is selected among a repository's versions.
            (Some(_), _) if self.version.is_some() => Err("says both a `version` and a `git`"),
            // The package there says its own.
            (Some(_), _) if self.id.is_some() => Err("says both an `id` and a `git`"),
            (Some(_), _) if states > 1 => {
                Err("says more than one of a `branch`, a `tag`, and a `rev`")
            }
            (Some(git), _) => {
                let source = Source {
                    git,
                    branch: self.branch,
                    tag: self.tag,
                    rev: self.rev,
                    directory: self.directory.unwrap_or_default(),
                };
                wrong(&source).map_or(Ok(Dependency::Git(source)), Err)
            }
            (None, _) if states > 0 => Err("says a `branch`, a `tag`, or a `rev`, and no `git`"),
            (None, _) if self.directory.is_some() => Err("says a `directory` and no `git`"),
            // The package at a directory is a member, as it is.
            (None, Some(_)) if self.version.is_some() => Err("says both a `path` and a `version`"),
            (None, Some(_)) if self.id.is_some() => Err("says both an `id` and a `path`"),
            (None, Some(path)) => Ok(Dependency::Path(path)),
            (None, None) => match (self.id, self.version) {
                (Some(id), Some(version)) => Ok(Dependency::Named { id, version }),
                (None, Some(_)) => Err("says a `version` and no `id`"),
                (_, None) => Err("says neither a `path`, a `version`, nor a `git`"),
            },
        }
    }
}

/// What is wrong with how a declaration says the state of its
/// repository, if anything.
fn wrong(source: &Source) -> Option<&'static str> {
    let digit = |byte: u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte);
    let whole = |rev: &String| matches!(rev.len(), 40 | 64) && rev.bytes().all(digit);
    let top = source.directory.as_os_str().is_empty();
    match &source.rev {
        // A name that moves is a branch or a tag.
        Some(rev) if !whole(rev) => Some("says a `rev` that is not a whole commit"),
        _ if !top && !inside(&source.directory) => {
            Some("says a `directory` that is not in the repository")
        }
        _ => None,
    }
}

//! Why a directory is not a project.

use std::fmt;
use std::path::PathBuf;

use super::Line;
use crate::syntax::SyntaxError;
use crate::workspace::WorkspaceError;

/// Why a directory is not a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectError {
    /// A file or directory could not be read.
    Unreadable {
        path: PathBuf,
        reason: String,
    },
    /// A manifest is not one.
    Manifest {
        path: PathBuf,
        reason: String,
    },
    /// A source file is not a module.
    Syntax {
        path: PathBuf,
        error: SyntaxError,
    },
    /// A source file's path does not name a module.
    Unnamed {
        path: PathBuf,
    },
    /// Two packages have one name.
    Duplicate {
        name: String,
        first: PathBuf,
        second: PathBuf,
    },
    /// A prelude names a package this one does not depend on.
    Prelude {
        package: String,
        prelude: String,
    },
    /// A build grant names no ability the package can reach.
    Grant {
        package: String,
        grant: String,
    },
    /// A build step or a lint, as `kind` says, names a target or a
    /// function the package cannot reach.
    Step {
        package: String,
        kind: &'static str,
        step: String,
        names: String,
    },
    /// Two members of a workspace are one package in one line of versions.
    Rivals {
        id: String,
        line: Line,
        first: PathBuf,
        second: PathBuf,
    },
    /// A dependency is on a package no member of the workspace is.
    Absent {
        package: String,
        name: String,
        id: String,
        line: Line,
    },
    /// A dependency says one thing of a package, and the package at
    /// the directory it names says another.
    Mistaken {
        package: String,
        name: String,
        path: PathBuf,
        declared: String,
        found: String,
    },
    /// A pinned dependency could not be fetched.
    Fetch {
        package: String,
        name: String,
        reason: String,
    },
    /// An update names no dependency on a branch or a tag.
    Fixed {
        package: String,
        name: String,
    },
    /// What a dependency leads to is not the snapshot the lock
    /// holds for it.
    Altered {
        /// Where it was read from.
        from: String,
        locked: String,
        found: String,
    },
    Workspace(WorkspaceError),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { path, reason } | Self::Manifest { path, reason } => {
                write!(f, "{}: {reason}", path.display())
            }
            Self::Syntax { path, error } => write!(f, "{}:{error}", path.display()),
            Self::Unnamed { path } => {
                write!(f, "{}: its path does not name a module", path.display())
            }
            Self::Duplicate {
                name,
                first,
                second,
            } => write!(
                f,
                "two packages are named `{name}`: {} and {}",
                first.display(),
                second.display()
            ),
            Self::Prelude { package, prelude } => {
                write!(
                    f,
                    "{package}: the prelude `{prelude}` names no package it can reach"
                )
            }
            Self::Grant { package, grant } => {
                write!(
                    f,
                    "{package}: the grant `{grant}` names no ability it can reach"
                )
            }
            Self::Step {
                package,
                kind,
                step,
                names,
            } => write!(
                f,
                "{package}: the {kind} `{step}` names `{names}`, which is no function or \
                 target it can reach"
            ),
            Self::Rivals {
                id,
                line,
                first,
                second,
            } => write!(
                f,
                "two members are package `{id}` in the version line `{line}`: {} and {}",
                first.display(),
                second.display()
            ),
            Self::Absent {
                package,
                name,
                id,
                line,
            } => write!(
                f,
                "{package}: the dependency `{name}` is on package `{id}` in the version line `{line}`, \
                 which no member of the workspace is"
            ),
            Self::Mistaken {
                package,
                name,
                path,
                declared,
                found,
            } => write!(
                f,
                "{package}: the dependency `{name}` says {declared}, and the package at {} is {found}",
                path.display()
            ),
            Self::Fetch {
                package,
                name,
                reason,
            } => write!(
                f,
                "{package}: the dependency `{name}` was not fetched: {reason}"
            ),
            Self::Fixed { package, name } => write!(
                f,
                "{package}: `{name}` is no dependency on a branch or a tag, so nothing updates it"
            ),
            Self::Altered {
                from,
                locked,
                found,
            } => write!(
                f,
                "{from} is not what was locked: its snapshot is {found}, and the lock holds {locked}"
            ),
            Self::Workspace(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ProjectError {}

//! Why a directory is not a project.

use std::fmt;
use std::path::PathBuf;

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
    /// Two members of a workspace are one package at one major.
    Rivals {
        id: String,
        major: u64,
        first: PathBuf,
        second: PathBuf,
    },
    /// A dependency is on a package no member of the workspace is.
    Absent {
        package: String,
        name: String,
        id: String,
        major: u64,
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
            Self::Rivals {
                id,
                major,
                first,
                second,
            } => write!(
                f,
                "two members are package `{id}` at major {major}: {} and {}",
                first.display(),
                second.display()
            ),
            Self::Absent {
                package,
                name,
                id,
                major,
            } => write!(
                f,
                "{package}: the dependency `{name}` is on package `{id}` at major {major}, \
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
            Self::Workspace(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ProjectError {}

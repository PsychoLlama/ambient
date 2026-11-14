//! A package as files: a manifest, and a module for each source file.
//! See `spec/packages.md` and `spec/modules.md`.
//!
//! A package is a directory holding `ambient.toml` and `src/`. Each
//! `.ab` file under `src/` is the module its path names, and `lib.ab`
//! is the library module. Files are reached through [`Files`], so a
//! project is assembled the same way from a disk and from memory.

mod files;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use ambient_runtime::FAULT_PACKAGE;
use serde::Deserialize;

pub use files::{Disk, Files};

use crate::builder::OWN_PACKAGE;
use crate::symbol::ModuleId;
use crate::syntax::SyntaxError;
use crate::workspace::{Workspace, WorkspaceError};

/// The name of a package's manifest.
pub const MANIFEST: &str = "ambient.toml";
/// The directory a package's modules are under.
const SOURCES: &str = "src";
/// The file that is a package's library module.
const LIBRARY: &str = "lib";
const EXTENSION: &str = "ab";

/// What a package says of itself.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// What the package is called in messages, and in this workspace.
    pub name: String,
    /// The package's identity, if it is not its name. See "Identity" in
    /// `spec/definitions.md`.
    pub id: Option<String>,
    /// The modules in scope in every module of the package, each a path
    /// that begins with `pkg` or a dependency. Listing any replaces the
    /// standard library's.
    #[serde(default)]
    pub preludes: Vec<String>,
    /// Other packages, by the name this one uses for each.
    #[serde(default)]
    pub dependencies: BTreeMap<String, Dependency>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    /// Where the package is, from the directory of this manifest.
    pub path: PathBuf,
}

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
            Self::Workspace(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ProjectError {}

/// A package and everything it depends on, ready to build.
#[derive(Clone, Debug)]
pub struct Project {
    pub workspace: Workspace,
    /// The package the project was opened at.
    pub root: String,
    /// Each module read from a file, and the file.
    pub modules: Vec<(ModuleId, PathBuf)>,
}

impl Project {
    /// The file a module was read from, if it was read from one.
    #[must_use]
    pub fn file(&self, module: &ModuleId) -> Option<&Path> {
        let found = self.modules.iter().find(|(id, _)| id == module);
        found.map(|(_, path)| path.as_path())
    }
}

/// Opens the package at `directory`, the packages it depends on, and
/// the standard library.
///
/// # Errors
///
/// Fails if a manifest or a source file is missing or malformed.
pub fn open(files: &dyn Files, directory: &Path) -> Result<Project, ProjectError> {
    let mut opening = Opening {
        files,
        workspace: Workspace::new(),
        opened: BTreeMap::new(),
        modules: Vec::new(),
    };
    crate::library::install(&mut opening.workspace).map_err(ProjectError::Workspace)?;
    let root = opening.package(directory)?;
    Ok(Project {
        workspace: opening.workspace,
        root,
        modules: opening.modules,
    })
}

struct Opening<'a> {
    files: &'a dyn Files,
    workspace: Workspace,
    /// Where each package opened so far is.
    opened: BTreeMap<String, PathBuf>,
    modules: Vec<(ModuleId, PathBuf)>,
}

/// `path` with `.` and `..` worked out, as far as they can be.
fn normal(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir
                if matches!(out.components().next_back(), Some(Component::Normal(_))) =>
            {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The module path a source file under a package's sources names, as
/// dotted segments.
fn module_path(relative: &Path) -> Option<String> {
    let named = relative.with_extension("");
    let mut segments = Vec::new();
    for component in named.components() {
        let Component::Normal(segment) = component else {
            return None;
        };
        let segment = segment.to_str()?;
        let mut chars = segment.chars();
        let starts = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_');
        if !starts || !chars.all(|c| c.is_alphanumeric() || c == '_') {
            return None;
        }
        segments.push(segment);
    }
    if segments == [LIBRARY] {
        segments.clear();
    }
    Some(segments.join("."))
}

impl Opening<'_> {
    fn read(&self, path: &Path) -> Result<String, ProjectError> {
        self.files
            .read(path)
            .map_err(|error| ProjectError::Unreadable {
                path: path.to_owned(),
                reason: error.to_string(),
            })
    }

    /// Opens the package at `directory`, if it is not open, and gives
    /// its name.
    fn package(&mut self, directory: &Path) -> Result<String, ProjectError> {
        let directory = normal(directory);
        let path = directory.join(MANIFEST);
        let manifest: Manifest =
            toml::from_str(&self.read(&path)?).map_err(|error| ProjectError::Manifest {
                path: path.clone(),
                reason: error.message().to_owned(),
            })?;
        let name = manifest.name.clone();
        match self.opened.get(&name) {
            Some(first) if *first == directory => return Ok(name),
            Some(first) => {
                return Err(ProjectError::Duplicate {
                    name,
                    first: first.clone(),
                    second: directory,
                });
            }
            None => self.opened.insert(name.clone(), directory.clone()),
        };
        if name == FAULT_PACKAGE || name == OWN_PACKAGE {
            let reason = format!("a package cannot be named `{name}`");
            return Err(ProjectError::Manifest { path, reason });
        }
        if let Some(id) = &manifest.id {
            self.workspace.identify(&name, id);
        }
        self.modules(&name, &directory.join(SOURCES))?;
        // Every package reaches the standard library by its name.
        let library = |error| ProjectError::Workspace(error);
        self.workspace
            .depend(&name, FAULT_PACKAGE, FAULT_PACKAGE)
            .map_err(library)?;
        for (local, dependency) in &manifest.dependencies {
            let package = self.package(&directory.join(&dependency.path))?;
            self.workspace
                .depend(&name, local, &package)
                .map_err(ProjectError::Workspace)?;
        }
        for prelude in &manifest.preludes {
            let module = self.prelude(&name, &manifest, prelude)?;
            self.workspace.prelude(&name, module);
        }
        Ok(name)
    }

    /// Reads every source file of a package into the workspace.
    fn modules(&mut self, package: &str, sources: &Path) -> Result<(), ProjectError> {
        let listed = self.files.sources(sources);
        let listed = listed.map_err(|error| ProjectError::Unreadable {
            path: sources.to_owned(),
            reason: error.to_string(),
        })?;
        for relative in listed {
            if relative.extension().and_then(|e| e.to_str()) != Some(EXTENSION) {
                continue;
            }
            let path = sources.join(&relative);
            let Some(module) = module_path(&relative) else {
                return Err(ProjectError::Unnamed { path });
            };
            let source = self.read(&path)?;
            let parsed = crate::syntax::parse(&source).map_err(|error| ProjectError::Syntax {
                path: path.clone(),
                error,
            })?;
            let id = ModuleId::new(package, &module);
            self.workspace
                .set(id.clone(), parsed)
                .map_err(ProjectError::Workspace)?;
            self.modules.push((id, path));
        }
        Ok(())
    }

    /// The module a prelude written in a manifest names.
    fn prelude(
        &self,
        package: &str,
        manifest: &Manifest,
        prelude: &str,
    ) -> Result<ModuleId, ProjectError> {
        let (first, rest) = prelude.split_once('.').unwrap_or((prelude, ""));
        let unknown = || ProjectError::Prelude {
            package: package.to_owned(),
            prelude: prelude.to_owned(),
        };
        if first == OWN_PACKAGE {
            return Ok(ModuleId::new(package, rest));
        }
        if first == FAULT_PACKAGE && !manifest.dependencies.contains_key(first) {
            return Ok(ModuleId::new(FAULT_PACKAGE, rest));
        }
        let dependency = manifest.dependencies.get(first).ok_or_else(unknown)?;
        let directory = normal(&self.opened[package].join(&dependency.path));
        let found = self.opened.iter().find(|(_, at)| **at == directory);
        let (name, _) = found.ok_or_else(unknown)?;
        Ok(ModuleId::new(name, rest))
    }
}

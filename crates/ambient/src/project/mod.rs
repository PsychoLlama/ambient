//! A package as files: a manifest, and a module for each source file.
//! See `docs/spec/packages.md` and `docs/spec/modules.md`.
//!
//! A package is a directory holding `ambient.toml` and `src/`. Each
//! `.ab` file under `src/` is the module its path names, and `lib.ab`
//! is the library module. Files are reached through [`Files`], so a
//! project is assembled the same way from a disk and from memory.

mod bins;
mod contents;
mod declare;
mod dependency;
mod error;
mod fetch;
mod files;
mod git;
mod listed;
mod locked;
mod manifest;
mod members;
mod opening;
mod settle;
mod vendor;
mod version;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use bins::{Bin, Binary, MAIN};
pub use contents::Contents;
pub use declare::declared;
pub use dependency::{Dependency, Granted};
pub use error::ProjectError;
pub use fetch::Kept;
pub use files::{Disk, Files};
use git::Git;
pub use locked::{
    Commit, LOCK, Lock, Locked, Nowhere, OFFLINE, Offline, Snapshots, Source, digest, fetched,
};
pub use manifest::{Bundle, Manifest, Package};
use members::{Members, manifest, normal};
pub use members::{apart, covered, named, nearest};
use opening::Opening;
pub use settle::Update;
pub use vendor::{Vendored, kept, vendored};
pub use version::{Line, Version};

use crate::error::Error;
use crate::surface::{Definition, GENERATOR};
use crate::symbol::{ModuleId, Symbol};
use crate::syntax::{Placed, Position};
use crate::workspace::{Sources, Workspace};

/// The name of a package's manifest.
pub const MANIFEST: &str = "ambient.toml";
/// The directory a package's modules are under.
pub(crate) const SOURCES: &str = "src";
/// The file that is a package's library module.
const LIBRARY: &str = "lib";
const EXTENSION: &str = "ab";

/// A package and everything it depends on, ready to build.
#[derive(Clone, Debug)]
pub struct Project {
    pub workspace: Workspace,
    /// The package the project was opened at.
    pub root: String,
    /// Each module read from a file.
    pub modules: Vec<Read>,
    /// The directory of the workspace's manifest, if the package is in
    /// one that lists its members.
    pub above: Option<PathBuf>,
    /// The workspace's lock and what it is to say from now on, if
    /// that is not what it says.
    pub locked: Option<(PathBuf, String)>,
    /// The files of each package that was opened, by its name.
    pub packages: BTreeMap<String, Contents>,
    /// The native bundles of each package that has any, by its name
    /// and then by the platform each is for.
    pub natives: BTreeMap<String, BTreeMap<String, Bundle>>,
    /// The binaries of the package at `root`, each by its name: what a
    /// command there runs and builds.
    pub bins: BTreeMap<String, Binary>,
}

/// A module read from a file.
#[derive(Clone, Debug)]
pub struct Read {
    pub module: ModuleId,
    pub file: PathBuf,
    /// Where each of the module's definitions is, in their order.
    pub definitions: Vec<Placed>,
}

impl Project {
    /// The file a definition is in and where it is there, if its module
    /// was read from one.
    #[must_use]
    pub fn locate(&self, symbol: &Symbol) -> Option<(&Path, &Placed)> {
        let read = self
            .modules
            .iter()
            .find(|read| read.module == symbol.module)?;
        let module = self.workspace.module(&read.module)?;
        let named = |definition: &Definition| {
            definition.name == symbol.name && definition.kind.namespace() == symbol.namespace
        };
        let index = module.definitions().iter().position(named)?;
        Some((read.file.as_path(), read.definitions.get(index)?))
    }

    /// Where the node numbered `node` of a function's body is: its
    /// file, line, and column, if the function was read from a file.
    /// A lambda's nodes are those of the definition it is written in.
    #[must_use]
    pub fn place(&self, function: &Symbol, node: usize) -> Option<String> {
        let (file, placed) = self.locate(function)?;
        let Position { line, column } = placed.nodes.get(node)?;
        Some(format!("{}:{line}:{column}", file.display()))
    }

    /// `error`, after the file and the place in it of what is wrong:
    /// the node of its definition's body numbered `node`, or else where
    /// the definition begins.
    #[must_use]
    pub fn report(&self, error: &Error, node: Option<usize>) -> String {
        let located = error.symbol().and_then(|symbol| {
            // What a generated module's call gave is nowhere in the
            // text: an error in it is at the call.
            let call = symbol.module.value(GENERATOR);
            let written = self.locate(symbol).map(|found| (found, node));
            written.or_else(|| self.locate(&call).map(|found| (found, None)))
        });
        let Some(((file, placed), node)) = located else {
            return error.to_string();
        };
        let written = node.and_then(|node| placed.nodes.get(node));
        let Position { line, column } = written.copied().unwrap_or(placed.at);
        format!("{}:{line}:{column}: {error}", file.display())
    }
}

/// Opens the package at `directory`, the packages it depends on, and
/// the standard library, on a machine that fetches nothing.
///
/// # Errors
///
/// Fails if a manifest or a source file is missing or malformed.
pub fn open(files: &dyn Files, directory: &Path) -> Result<Project, ProjectError> {
    open_from(files, &Nowhere, directory)
}

/// [`open`], with the snapshots of dependencies in repositories kept
/// in, and fetched by, `snapshots`.
///
/// # Errors
///
/// Fails if a manifest or a source file is missing or malformed, or
/// what a dependency leads to is not what its lock holds.
pub fn open_from(
    files: &dyn Files,
    snapshots: &dyn Snapshots,
    directory: &Path,
) -> Result<Project, ProjectError> {
    open_with(files, snapshots, directory, &Update::Nothing)
}

/// [`open_from`], reading again the branches and tags `update` says.
///
/// # Errors
///
/// As [`open_from`], or `update` names no dependency that moves.
pub fn open_with(
    files: &dyn Files,
    snapshots: &dyn Snapshots,
    directory: &Path,
    update: &Update,
) -> Result<Project, ProjectError> {
    let (project, _) = open_members(files, snapshots, &[directory.to_owned()], update)?;
    Ok(project)
}

/// The members at `directories` opened into one project, whose root is
/// the first, with the name of each: what one program of several
/// members, as their tests, is built over.
///
/// # Errors
///
/// As [`open_with`].
pub fn open_members(
    files: &dyn Files,
    snapshots: &dyn Snapshots,
    directories: &[PathBuf],
    update: &Update,
) -> Result<(Project, Vec<String>), ProjectError> {
    let first = directories.first().map(|directory| normal(directory));
    let first = first.unwrap_or_default();
    let settled = settle::settled(files, snapshots, &first, update)?;
    let mut opening = opening::Opening::new(files, snapshots, settled.members, settled.lock);
    crate::library::install(&mut opening.workspace).map_err(ProjectError::Workspace)?;
    crate::system::install(&mut opening.workspace).map_err(ProjectError::Workspace)?;
    let mut names = Vec::new();
    for directory in directories {
        names.push(opening.member(&normal(directory))?);
    }
    opening.hold_vendored()?;
    let root = names.first().cloned().unwrap_or_default();
    let bins = opening.bins.remove(&root).unwrap_or_default();
    let project = Project {
        bins,
        above: opening.members.root,
        locked: settled.written,
        workspace: opening.workspace,
        root,
        modules: opening.modules,
        packages: opening.opened,
        natives: opening.natives,
    };
    Ok((project, names))
}

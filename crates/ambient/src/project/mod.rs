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
mod settle;
mod vendor;
mod version;

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use ambient_host::hash::Hash;
use ambient_runtime::FAULT_PACKAGE;
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
pub use settle::Update;
pub use vendor::{Vendored, kept, vendored};
pub use version::{Line, Version};

use crate::builder::OWN_PACKAGE;
use crate::error::Error;
use crate::surface::{Definition, GENERATOR};
use crate::symbol::PackageId;
use crate::symbol::{ModuleId, Namespace, Symbol};
use crate::syntax::{Placed, Position};
use crate::workspace::{Granting, Sources, Workspace};

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
    let directory = normal(directory);
    let settled = settle::settled(files, snapshots, &directory, update)?;
    let mut opening = Opening {
        files,
        snapshots,
        members: settled.members,
        lock: settled.lock,
        workspace: Workspace::new(),
        opened: BTreeMap::new(),
        natives: BTreeMap::new(),
        bins: BTreeMap::new(),
        modules: Vec::new(),
    };
    crate::library::install(&mut opening.workspace).map_err(ProjectError::Workspace)?;
    crate::system::install(&mut opening.workspace).map_err(ProjectError::Workspace)?;
    let root = opening.member(&directory)?;
    opening.hold_vendored()?;
    let bins = opening.bins.remove(&root).unwrap_or_default();
    Ok(Project {
        bins,
        above: opening.members.root,
        locked: settled.written,
        workspace: opening.workspace,
        root,
        modules: opening.modules,
        packages: opening.opened,
        natives: opening.natives,
    })
}

struct Opening<'a> {
    files: &'a dyn Files,
    snapshots: &'a dyn Snapshots,
    /// The packages of the workspace, opened or not.
    members: Members,
    /// What the workspace locks, settled.
    lock: Lock,
    workspace: Workspace,
    /// The files of each package opened so far.
    opened: BTreeMap<String, Contents>,
    /// The native bundles of each package opened so far that has any.
    natives: BTreeMap<String, BTreeMap<String, Bundle>>,
    /// The binaries of each package opened so far, by the package.
    bins: BTreeMap<String, BTreeMap<String, Binary>>,
    modules: Vec<Read>,
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

    /// Opens the member at `directory`, if it is not open, and gives
    /// its name.
    fn member(&mut self, directory: &Path) -> Result<String, ProjectError> {
        let member = self.members.at(self.files, directory)?.clone();
        self.package(member.directory, &member.package, None)
    }

    /// Opens a package, if it is not open, and gives the name of the
    /// instance: the package's own for a member, and one that tells a
    /// snapshot from every other.
    fn package(
        &mut self,
        directory: PathBuf,
        manifest: &Package,
        snapshot: Option<Hash>,
    ) -> Result<String, ProjectError> {
        let path = directory.join(MANIFEST);
        let name = match snapshot {
            Some(snapshot) => format!("{}@{}", manifest.name, &snapshot.to_string()[..8]),
            None => manifest.name.clone(),
        };
        match self.opened.get(&name).map(|first| &first.directory) {
            Some(first) if Members::same(self.files, first, &directory) => return Ok(name),
            Some(first) => {
                return Err(ProjectError::Duplicate {
                    name,
                    first: first.clone(),
                    second: directory,
                });
            }
            None => {
                let contents = Contents {
                    directory: directory.clone(),
                    listed: manifest.files.clone(),
                };
                self.opened.insert(name.clone(), contents);
            }
        }
        let taken = [FAULT_PACKAGE, crate::system::PACKAGE];
        if taken.contains(&manifest.name.as_str()) || manifest.name == OWN_PACKAGE {
            let reason = format!("a package cannot be named `{}`", manifest.name);
            return Err(ProjectError::Manifest { path, reason });
        }
        // What comes with the compiler is named by its ID: no package
        // takes one.
        if manifest.id.is_empty() || taken.contains(&manifest.id.as_str()) {
            let reason = format!("a package's ID cannot be `{}`", manifest.id);
            return Err(ProjectError::Manifest { path, reason });
        }
        self.workspace.identify(&name, &manifest.id);
        // A snapshot is no code of the workspace's: it takes what
        // those that declare it grant.
        if snapshot.is_some() {
            self.workspace.hold(&name);
        }
        if !manifest.native.is_empty() {
            self.natives.insert(name.clone(), manifest.native.clone());
        }
        self.modules(&name, &directory.join(SOURCES))?;
        // Every package reaches the standard library by its name.
        let library = |error| ProjectError::Workspace(error);
        for given in taken {
            self.workspace
                .depend(&name, given, given)
                .map_err(library)?;
        }
        // A package that depends on this one finds it open already,
        // so packages may depend on each other.
        for (local, dependency) in &manifest.dependencies {
            let package = match dependency {
                Dependency::Git(source) => self.git(&Git {
                    package: &name,
                    name: local,
                    source,
                })?,
                // A snapshot is whole: nothing is beside it.
                Dependency::Path(_) if snapshot.is_some() => {
                    let reason = format!("the dependency `{local}` names a directory");
                    return Err(ProjectError::Manifest { path, reason });
                }
                Dependency::Path(at) => self.member(&normal(&directory.join(at)))?,
                Dependency::Named { id, version } => {
                    let line = Line::of(version);
                    let standing = self.members.standing(id, line);
                    let standing = standing.map(|member| member.directory.clone());
                    let standing = standing.ok_or_else(|| ProjectError::Absent {
                        package: name.clone(),
                        name: local.clone(),
                        id: id.clone(),
                        line,
                    })?;
                    self.member(&standing)?
                }
            };
            self.workspace
                .depend(&name, local, &package)
                .map_err(ProjectError::Workspace)?;
            let granted = &manifest.granted[local];
            self.workspace.granting(Granting {
                from: PackageId(name.clone()),
                local: local.clone(),
                to: PackageId(package),
                abilities: granted.abilities.clone(),
                native: granted.native,
            });
        }
        for prelude in &manifest.preludes {
            let module = self.prelude(&name, prelude)?;
            self.workspace.prelude(&name, module);
        }
        self.tools(&name, manifest)?;
        let bins = self.binaries(&name, &path, manifest)?;
        self.bins.insert(name.clone(), bins);
        Ok(name)
    }

    /// Declares the tools the manifest of the package `name` lists: its
    /// build steps, its lints, and its ways to find tests.
    fn tools(&mut self, name: &str, manifest: &Package) -> Result<(), ProjectError> {
        for step in &manifest.steps {
            let named = |path| self.value(name, path, "build step", &step.tool);
            let (target, tool) = (named(&step.target)?, named(&step.tool)?);
            self.workspace.step(name, target, tool);
        }
        for lint in &manifest.lints {
            let tool = self.value(name, lint, "lint", lint)?;
            self.workspace.lint(name, tool);
        }
        for finder in &manifest.test.find {
            let tool = self.value(name, finder, "way to find tests", finder)?;
            self.workspace.finder(name, tool);
        }
        if let Some(runner) = &manifest.test.run {
            let runner = self.value(name, runner, "way to run tests", runner)?;
            self.workspace.run_by(name, runner);
        }
        Ok(())
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
            let read = crate::syntax::read(&source).map_err(|error| ProjectError::Syntax {
                path: path.clone(),
                error,
            })?;
            let id = ModuleId::new(package, &module);
            self.workspace
                .set(id.clone(), read.module)
                .map_err(ProjectError::Workspace)?;
            self.modules.push(Read {
                module: id,
                file: path,
                definitions: read.definitions,
            });
        }
        Ok(())
    }

    /// Holds each member that was vendored to what it is granted, as
    /// the package it was copied from would be.
    fn hold_vendored(&mut self) -> Result<(), ProjectError> {
        let Some(base) = self.members.root.clone() else {
            return Ok(());
        };
        for path in manifest(self.files, &base)?.vendored.into_keys() {
            let directory = normal(&base.join(path));
            let copied = |opened: &(&String, &Contents)| {
                Members::same(self.files, &opened.1.directory, &directory)
            };
            if let Some((name, _)) = self.opened.iter().find(copied) {
                self.workspace.hold(name);
            }
        }
        Ok(())
    }

    /// The function or the target that the build step or the lint
    /// `step`, which is a `kind`, names at `path`: one some module that
    /// was opened declares.
    fn value(
        &self,
        package: &str,
        path: &str,
        kind: &'static str,
        step: &str,
    ) -> Result<Symbol, ProjectError> {
        let unknown = || ProjectError::Step {
            package: package.to_owned(),
            kind,
            step: step.to_owned(),
            names: path.to_owned(),
        };
        let (module, name) = path.rsplit_once('.').ok_or_else(unknown)?;
        let module = self.prelude(package, module).map_err(|_| unknown())?;
        let declared = self.workspace.module(&module).is_some_and(|module| {
            let named = |definition: &Definition| {
                definition.name == name && definition.kind.namespace() == Namespace::Value
            };
            module.definitions().iter().any(named)
        });
        declared.then(|| module.value(name)).ok_or_else(unknown)
    }

    /// The module a prelude written in a manifest names: a path that
    /// begins as one in the package's modules does.
    fn prelude(&self, package: &str, prelude: &str) -> Result<ModuleId, ProjectError> {
        let (first, rest) = prelude.split_once('.').unwrap_or((prelude, ""));
        if first == OWN_PACKAGE {
            return Ok(ModuleId::new(package, rest));
        }
        let own = PackageId(package.to_owned());
        let named = self.workspace.dependency(&own, first);
        let named = named.ok_or_else(|| ProjectError::Prelude {
            package: package.to_owned(),
            prelude: prelude.to_owned(),
        })?;
        Ok(ModuleId::new(&named.0, rest))
    }
}

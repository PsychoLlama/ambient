//! Opening a project: each package a member reaches, read into one
//! workspace. See "Packages" in `docs/spec/packages.md`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use ambient_host::hash::Hash;
use ambient_runtime::FAULT_PACKAGE;

use super::{
    Binary, Bundle, Contents, Dependency, EXTENSION, Files, Git, LIBRARY, Line, Lock, MANIFEST,
    Members, Package, ProjectError, Read, SOURCES, Snapshots, manifest, normal,
};
use crate::builder::OWN_PACKAGE;
use crate::surface::Definition;
use crate::symbol::{ModuleId, Namespace, PackageId, Symbol};
use crate::workspace::{Granting, Sources, Workspace};

pub(super) struct Opening<'a> {
    pub(super) files: &'a dyn Files,
    pub(super) snapshots: &'a dyn Snapshots,
    /// The packages of the workspace, opened or not.
    pub(super) members: Members,
    /// What the workspace locks, settled.
    pub(super) lock: Lock,
    pub(super) workspace: Workspace,
    /// The files of each package opened so far.
    pub(super) opened: BTreeMap<String, Contents>,
    /// The native bundles of each package opened so far that has any.
    pub(super) natives: BTreeMap<String, BTreeMap<String, Bundle>>,
    /// The binaries of each package opened so far, by the package.
    pub(super) bins: BTreeMap<String, BTreeMap<String, Binary>>,
    pub(super) modules: Vec<Read>,
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

impl<'a> Opening<'a> {
    pub(super) fn new(
        files: &'a dyn Files,
        snapshots: &'a dyn Snapshots,
        members: Members,
        lock: Lock,
    ) -> Self {
        Self {
            files,
            snapshots,
            members,
            lock,
            workspace: Workspace::new(),
            opened: BTreeMap::new(),
            natives: BTreeMap::new(),
            bins: BTreeMap::new(),
            modules: Vec::new(),
        }
    }

    pub(super) fn read(&self, path: &Path) -> Result<String, ProjectError> {
        self.files
            .read(path)
            .map_err(|error| ProjectError::Unreadable {
                path: path.to_owned(),
                reason: error.to_string(),
            })
    }

    /// Opens the member at `directory`, if it is not open, and gives
    /// its name.
    pub(super) fn member(&mut self, directory: &Path) -> Result<String, ProjectError> {
        let member = self.members.at(self.files, directory)?.clone();
        self.package(member.directory, &member.package, None)
    }

    /// Opens a package, if it is not open, and gives the name of the
    /// instance: the package's own for a member, and one that tells a
    /// snapshot from every other.
    pub(super) fn package(
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
    pub(super) fn tools(&mut self, name: &str, manifest: &Package) -> Result<(), ProjectError> {
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
    pub(super) fn modules(&mut self, package: &str, sources: &Path) -> Result<(), ProjectError> {
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
    pub(super) fn hold_vendored(&mut self) -> Result<(), ProjectError> {
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
    pub(super) fn value(
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
    pub(super) fn prelude(&self, package: &str, prelude: &str) -> Result<ModuleId, ProjectError> {
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

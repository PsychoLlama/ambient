//! A package as files: a manifest, and a module for each source file.
//! See `spec/packages.md` and `spec/modules.md`.
//!
//! A package is a directory holding `ambient.toml` and `src/`. Each
//! `.ab` file under `src/` is the module its path names, and `lib.ab`
//! is the library module. Files are reached through [`Files`], so a
//! project is assembled the same way from a disk and from memory.

mod error;
mod files;
mod manifest;
mod members;

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use ambient_runtime::FAULT_PACKAGE;
pub use error::ProjectError;
pub use files::{Disk, Files};
pub use manifest::{Dependency, Manifest, Package, Version};
use members::{Members, normal};

use crate::builder::OWN_PACKAGE;
use crate::error::Error;
use crate::surface::{Definition, GENERATOR};
use crate::symbol::PackageId;
use crate::symbol::{ModuleId, Namespace, Symbol};
use crate::syntax::{Placed, Position};
use crate::workspace::{Sources, Workspace};

/// The name of a package's manifest.
pub const MANIFEST: &str = "ambient.toml";
/// The directory a package's modules are under.
const SOURCES: &str = "src";
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
    /// Where each package that was opened is, by its name.
    pub directories: BTreeMap<String, PathBuf>,
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
/// the standard library.
///
/// # Errors
///
/// Fails if a manifest or a source file is missing or malformed.
pub fn open(files: &dyn Files, directory: &Path) -> Result<Project, ProjectError> {
    let directory = normal(directory);
    let mut opening = Opening {
        files,
        members: Members::around(files, &directory)?,
        workspace: Workspace::new(),
        opened: BTreeMap::new(),
        modules: Vec::new(),
    };
    crate::library::install(&mut opening.workspace).map_err(ProjectError::Workspace)?;
    crate::system::install(&mut opening.workspace).map_err(ProjectError::Workspace)?;
    let root = opening.package(&directory)?;
    Ok(Project {
        workspace: opening.workspace,
        root,
        modules: opening.modules,
        directories: opening.opened,
    })
}

struct Opening<'a> {
    files: &'a dyn Files,
    /// The packages of the workspace, opened or not.
    members: Members,
    workspace: Workspace,
    /// Where each package opened so far is.
    opened: BTreeMap<String, PathBuf>,
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

    /// Opens the package at `directory`, if it is not open, and gives
    /// its name.
    fn package(&mut self, directory: &Path) -> Result<String, ProjectError> {
        let member = self.members.at(self.files, directory)?.clone();
        let (directory, manifest) = (member.directory, member.package);
        let path = directory.join(MANIFEST);
        let name = manifest.name.clone();
        match self.opened.get(&name) {
            Some(first) if Members::same(self.files, first, &directory) => return Ok(name),
            Some(first) => {
                return Err(ProjectError::Duplicate {
                    name,
                    first: first.clone(),
                    second: directory,
                });
            }
            None => self.opened.insert(name.clone(), directory.clone()),
        };
        let taken = [FAULT_PACKAGE, crate::system::PACKAGE];
        if taken.contains(&name.as_str()) || name == OWN_PACKAGE {
            let reason = format!("a package cannot be named `{name}`");
            return Err(ProjectError::Manifest { path, reason });
        }
        // What comes with the compiler is named by its ID: no package
        // takes one.
        if manifest.id.is_empty() || taken.contains(&manifest.id.as_str()) {
            let reason = format!("a package's ID cannot be `{}`", manifest.id);
            return Err(ProjectError::Manifest { path, reason });
        }
        self.workspace.identify(&name, &manifest.id);
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
            let target = self.target(&name, &directory, local, dependency)?;
            let package = self.package(&target)?;
            self.workspace
                .depend(&name, local, &package)
                .map_err(ProjectError::Workspace)?;
        }
        for prelude in &manifest.preludes {
            let module = self.prelude(&name, prelude)?;
            self.workspace.prelude(&name, module);
        }
        for grant in &manifest.grants {
            let ability = self.grant(&name, grant)?;
            self.workspace.grant(&name, ability);
        }
        Ok(name)
    }

    /// Where the package a dependency of `package` means is: the
    /// directory it names, or the member that stands in for it.
    fn target(
        &mut self,
        package: &str,
        directory: &Path,
        name: &str,
        dependency: &Dependency,
    ) -> Result<PathBuf, ProjectError> {
        let Some(path) = &dependency.path else {
            // A manifest is not read without one or the other.
            let major = dependency.version.map_or(0, |version| version.major);
            let standing = self.members.standing(&dependency.id, major);
            return standing
                .map(|member| member.directory.clone())
                .ok_or_else(|| ProjectError::Absent {
                    package: package.to_owned(),
                    name: name.to_owned(),
                    id: dependency.id.clone(),
                    major,
                });
        };
        let path = normal(&directory.join(path));
        let found = &self.members.at(self.files, &path)?.package;
        let mistaken = |declared: String, found: String| ProjectError::Mistaken {
            package: package.to_owned(),
            name: name.to_owned(),
            path: path.clone(),
            declared,
            found,
        };
        if found.id != dependency.id {
            let (declared, found) = (&dependency.id, &found.id);
            return Err(mistaken(
                format!("package `{declared}`"),
                format!("`{found}`"),
            ));
        }
        match dependency.version {
            Some(least) if least.major != found.version.major || least > found.version => {
                Err(mistaken(
                    format!("at least version {least}"),
                    format!("version {}", found.version),
                ))
            }
            _ => Ok(path),
        }
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

    /// The ability a build grant names, which is one some module that
    /// was opened declares.
    fn grant(&self, package: &str, grant: &str) -> Result<Symbol, ProjectError> {
        let unknown = || ProjectError::Grant {
            package: package.to_owned(),
            grant: grant.to_owned(),
        };
        let (module, name) = grant.rsplit_once('.').ok_or_else(unknown)?;
        let module = self.prelude(package, module);
        let module = module.map_err(|_| unknown())?;
        let declared = self.workspace.module(&module).is_some_and(|module| {
            let ability = |definition: &Definition| {
                definition.name == name && definition.kind.namespace() == Namespace::Ability
            };
            module.definitions().iter().any(ability)
        });
        declared.then(|| module.ability(name)).ok_or_else(unknown)
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

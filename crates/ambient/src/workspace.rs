//! Where an engine reads a program from.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::builder::OWN_PACKAGE;
use crate::surface::Module;
use crate::symbol::{ModuleId, ModulePath, PackageId};

/// One unchanging snapshot of a program's sources.
///
/// The engine reads everything through this, so a test supplies a program
/// from memory and nothing touches a disk.
pub trait Sources {
    fn module(&self, id: &ModuleId) -> Option<Arc<Module>>;

    /// The package that `package` reaches under the local name `name`.
    fn dependency(&self, package: &PackageId, name: &str) -> Option<PackageId>;

    /// The preludes of `package`, in the order it lists them.
    fn preludes(&self, package: &PackageId) -> Vec<ModuleId>;
}

/// Why a workspace refused a change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceError {
    /// A path segment names a file or a directory, never both.
    FileAndDirectory(ModuleId),
    /// `pkg` always means a module's own package.
    ReservedName,
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileAndDirectory(id) => write!(f, "`{id}` is both a module and a directory"),
            Self::ReservedName => write!(f, "`{OWN_PACKAGE}` cannot name a dependency"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

/// A program held in memory. Cloning is cheap, and an edit is a change to a
/// clone.
#[derive(Clone, Debug, Default)]
pub struct Workspace {
    modules: BTreeMap<ModuleId, Arc<Module>>,
    dependencies: BTreeMap<(PackageId, String), PackageId>,
    preludes: BTreeMap<PackageId, Vec<ModuleId>>,
}

impl Workspace {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a module, or replaces the one at `id`.
    ///
    /// # Errors
    ///
    /// Fails if `id` would be both a module and a directory holding modules.
    pub fn set(&mut self, id: ModuleId, module: Module) -> Result<(), WorkspaceError> {
        let clash = self
            .modules
            .keys()
            .filter(|other| other.package == id.package && **other != id)
            // The library module sits beside the top-level modules, not
            // above them.
            .filter(|other| !other.path.0.is_empty() && !id.path.0.is_empty())
            .any(|other| is_prefix(&other.path, &id.path) || is_prefix(&id.path, &other.path));
        if clash {
            return Err(WorkspaceError::FileAndDirectory(id));
        }
        self.modules.insert(id, Arc::new(module));
        Ok(())
    }

    pub fn remove(&mut self, id: &ModuleId) {
        self.modules.remove(id);
    }

    /// Lets modules of `package` reach `target` under the local name `name`.
    ///
    /// # Errors
    ///
    /// Fails if `name` is reserved.
    pub fn depend(
        &mut self,
        package: &str,
        name: &str,
        target: &str,
    ) -> Result<(), WorkspaceError> {
        if name == OWN_PACKAGE {
            return Err(WorkspaceError::ReservedName);
        }
        self.dependencies.insert(
            (PackageId(package.to_owned()), name.to_owned()),
            PackageId(target.to_owned()),
        );
        Ok(())
    }
}

impl Workspace {
    /// Adds `module` to the end of the preludes of `package`: its public
    /// definitions and re-exports are then in scope in every module of the
    /// package. See `spec/modules.md`.
    pub fn prelude(&mut self, package: &str, module: ModuleId) {
        self.preludes
            .entry(PackageId(package.to_owned()))
            .or_default()
            .push(module);
    }
}

fn is_prefix(shorter: &ModulePath, longer: &ModulePath) -> bool {
    shorter.0.len() < longer.0.len() && longer.0.starts_with(&shorter.0)
}

impl Sources for Workspace {
    fn module(&self, id: &ModuleId) -> Option<Arc<Module>> {
        self.modules.get(id).cloned()
    }

    fn dependency(&self, package: &PackageId, name: &str) -> Option<PackageId> {
        self.dependencies
            .get(&(package.clone(), name.to_owned()))
            .cloned()
    }

    fn preludes(&self, package: &PackageId) -> Vec<ModuleId> {
        self.preludes.get(package).cloned().unwrap_or_default()
    }
}

//! Where an engine reads a program from.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::builder::OWN_PACKAGE;
use crate::surface::Module;
use crate::symbol::{ModuleId, ModulePath, PackageId, Symbol};

/// One unchanging snapshot of a program's sources.
///
/// The engine reads everything through this, so a test supplies a program
/// from memory and nothing touches a disk.
pub trait Sources {
    fn module(&self, id: &ModuleId) -> Option<Arc<Module>>;

    /// Every module of `package`, in order of path.
    fn modules(&self, package: &PackageId) -> Vec<ModuleId>;

    /// The package that `package` reaches under the local name `name`.
    fn dependency(&self, package: &PackageId, name: &str) -> Option<PackageId>;

    /// The preludes of `package`, in the order it lists them.
    fn preludes(&self, package: &PackageId) -> Vec<ModuleId>;

    /// The ID of the package that `package` is an instance of: what its
    /// tags are scoped by. Every snapshot of one package has the same ID,
    /// whatever a workspace calls it. See `spec/packages.md`.
    fn identity(&self, package: &PackageId) -> String;

    /// The build abilities `package` is granted: what its comptime
    /// calls may take from the build target. See "Authority" in
    /// `spec/comptime.md`.
    fn grants(&self, package: &PackageId) -> Vec<Symbol>;

    /// Every build step a package declares: the target it is for and
    /// the tool it runs, in an order that is the same on every build.
    /// See "Builds" in `spec/tools.md`.
    fn steps(&self) -> Vec<(Symbol, Symbol)>;
}

/// Why a workspace refused a change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceError {
    /// A path segment names a file or a directory, never both.
    FileAndDirectory(ModuleId),
    /// `pkg` always means a module's own package.
    ReservedName,
    /// The standard library could not be built, and why.
    Library(String),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileAndDirectory(id) => write!(f, "`{id}` is both a module and a directory"),
            Self::ReservedName => write!(f, "`{OWN_PACKAGE}` cannot name a dependency"),
            Self::Library(why) => write!(f, "the standard library does not build: {why}"),
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
    identities: BTreeMap<PackageId, String>,
    grants: BTreeMap<PackageId, Vec<Symbol>>,
    steps: BTreeMap<PackageId, Vec<(Symbol, Symbol)>>,
    /// The preludes of a package that lists none.
    shared: Vec<ModuleId>,
    /// The standard library's preludes, for when `shared` is empty too.
    standard: Vec<ModuleId>,
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
    /// Says which package `package` is an instance of. One that is never
    /// told is a package of its own, with its name for an ID.
    pub fn identify(&mut self, package: &str, id: &str) {
        self.identities
            .insert(PackageId(package.to_owned()), id.to_owned());
    }

    /// Grants `package` a build ability: its comptime calls may take
    /// `ability` from the build target.
    pub fn grant(&mut self, package: &str, ability: Symbol) {
        self.grants
            .entry(PackageId(package.to_owned()))
            .or_default()
            .push(ability);
    }

    /// Declares a build step of `package`: `tool` adds files to a
    /// build under `target`, or under a target that extends it.
    pub fn step(&mut self, package: &str, target: Symbol, tool: Symbol) {
        self.steps
            .entry(PackageId(package.to_owned()))
            .or_default()
            .push((target, tool));
    }

    /// Adds `module` to the end of the workspace's own preludes, which a
    /// package that lists none takes.
    pub fn shared_prelude(&mut self, module: ModuleId) {
        self.shared.push(module);
    }

    /// Sets the standard library's preludes, which a package takes if
    /// neither it nor the workspace lists any.
    pub(crate) fn standard(&mut self, preludes: Vec<ModuleId>) {
        self.standard = preludes;
    }

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

    fn modules(&self, package: &PackageId) -> Vec<ModuleId> {
        let of_package = |id: &&ModuleId| id.package == *package;
        self.modules.keys().filter(of_package).cloned().collect()
    }

    fn dependency(&self, package: &PackageId, name: &str) -> Option<PackageId> {
        self.dependencies
            .get(&(package.clone(), name.to_owned()))
            .cloned()
    }

    fn preludes(&self, package: &PackageId) -> Vec<ModuleId> {
        if let Some(listed) = self.preludes.get(package) {
            return listed.clone();
        }
        if !self.shared.is_empty() {
            return self.shared.clone();
        }
        // The standard library is not a prelude of itself.
        let outside = |prelude: &&ModuleId| prelude.package != *package;
        self.standard.iter().filter(outside).cloned().collect()
    }

    fn identity(&self, package: &PackageId) -> String {
        self.identities.get(package).unwrap_or(&package.0).clone()
    }

    fn grants(&self, package: &PackageId) -> Vec<Symbol> {
        self.grants.get(package).cloned().unwrap_or_default()
    }

    fn steps(&self) -> Vec<(Symbol, Symbol)> {
        // By the ID of the package, which no workspace's names change,
        // and then as its manifest lists them.
        let mut packages: Vec<_> = self.steps.iter().collect();
        packages.sort_by_key(|(package, _)| self.identity(package));
        let steps = packages.into_iter().flat_map(|(_, steps)| steps);
        steps.cloned().collect()
    }
}

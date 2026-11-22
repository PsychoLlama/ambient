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
    /// whatever a workspace calls it. See `docs/spec/packages.md`.
    fn identity(&self, package: &PackageId) -> String;

    /// What `package` is granted outright, if it is held to what it is
    /// granted: a package that is not is the workspace's own, and
    /// takes anything. See "Grants" in `docs/spec/packages.md`.
    fn holds(&self, package: &PackageId) -> Option<Held>;

    /// Every package that is held.
    fn held(&self) -> Vec<PackageId>;

    /// Every grant a package makes one it depends on, in an order that
    /// is the same on every build.
    fn grants(&self) -> Vec<Granting>;

    /// Every build step a package declares: the target it is for and
    /// the tool it runs, in an order that is the same on every build.
    /// See "Builds" in `docs/spec/tools.md`.
    fn steps(&self) -> Vec<(Symbol, Symbol)>;

    /// The lints checking `package` runs, as its manifest lists them.
    /// See "Commands" in `docs/spec/tools.md`.
    fn lints(&self, package: &PackageId) -> Vec<Symbol>;

    /// The functions that find the tests of `package`, as its manifest
    /// lists them: none if it lists none. See "Finding" in
    /// `docs/spec/testing.md`.
    fn finders(&self, package: &PackageId) -> Vec<Symbol>;
}

/// What a package that is held is granted by its workspace itself,
/// whatever any package grants it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub abilities: Vec<Symbol>,
    /// Whether a program may reach native code of it.
    pub native: bool,
}

/// What one package lets another take: a dependency declaration's
/// grants, as its manifest writes them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Granting {
    /// The package whose manifest says so.
    pub from: PackageId,
    /// What that package calls the dependency.
    pub local: String,
    pub to: PackageId,
    /// Each a path as a row's is, from a module of `from`: an ability,
    /// or a module for every public ability of it.
    pub abilities: Vec<String>,
    /// Whether the dependency may bring native code.
    pub native: bool,
}

/// The packages that may bring native code, of those that are held:
/// each the workspace says may, and each a declaration says may whose
/// own package may. A grant of it is cut as any is, and it names no
/// ability, so it is known with no module read.
fn bringing(sources: &dyn Sources) -> Vec<PackageId> {
    let outright = |package: &PackageId| sources.holds(package).is_some_and(|held| held.native);
    let mut may: Vec<PackageId> = sources.held().into_iter().filter(outright).collect();
    let grants = sources.grants();
    loop {
        let allowed =
            |package: &PackageId| sources.holds(package).is_none() || may.contains(package);
        let passed =
            |grant: &&Granting| grant.native && allowed(&grant.from) && !allowed(&grant.to);
        let Some(grant) = grants.iter().find(passed) else {
            return may;
        };
        may.push(grant.to.clone());
    }
}

/// Whether a program may reach native code of `package`. See "Grants"
/// in `docs/spec/packages.md`.
#[must_use]
pub fn native(sources: &dyn Sources, package: &PackageId) -> bool {
    sources.holds(package).is_none() || bringing(sources).contains(package)
}

/// The declarations that could let `package` bring native code: those
/// of packages that may themselves.
#[must_use]
pub fn could_allow(sources: &dyn Sources, package: &PackageId) -> Vec<Granting> {
    let leads = |grant: &Granting| grant.to == *package && native(sources, &grant.from);
    sources.grants().into_iter().filter(leads).collect()
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
    held: BTreeMap<PackageId, Held>,
    grants: Vec<Granting>,
    steps: BTreeMap<PackageId, Vec<(Symbol, Symbol)>>,
    lints: BTreeMap<PackageId, Vec<Symbol>>,
    finders: BTreeMap<PackageId, Vec<Symbol>>,
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

    /// Holds `package` to what it is granted from now on: it is no
    /// code of the workspace's own.
    pub fn hold(&mut self, package: &str) -> &mut Held {
        self.held.entry(PackageId(package.to_owned())).or_default()
    }

    /// Grants `package` an ability outright, and holds it to what it
    /// is granted.
    pub fn grant(&mut self, package: &str, ability: Symbol) {
        self.hold(package).abilities.push(ability);
    }

    /// Says what a package lets one it depends on take.
    pub fn granting(&mut self, granting: Granting) {
        self.grants.push(granting);
    }

    /// Declares a lint of `package`: checking it runs `tool`.
    pub fn lint(&mut self, package: &str, tool: Symbol) {
        let package = PackageId(package.to_owned());
        self.lints.entry(package).or_default().push(tool);
    }

    /// Declares a way to find the tests of `package`: `tool`.
    pub fn finder(&mut self, package: &str, tool: Symbol) {
        let package = PackageId(package.to_owned());
        self.finders.entry(package).or_default().push(tool);
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
    /// package. See `docs/spec/modules.md`.
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

    fn lints(&self, package: &PackageId) -> Vec<Symbol> {
        self.lints.get(package).cloned().unwrap_or_default()
    }

    fn finders(&self, package: &PackageId) -> Vec<Symbol> {
        self.finders.get(package).cloned().unwrap_or_default()
    }

    fn holds(&self, package: &PackageId) -> Option<Held> {
        self.held.get(package).cloned()
    }

    fn held(&self) -> Vec<PackageId> {
        self.held.keys().cloned().collect()
    }

    fn grants(&self) -> Vec<Granting> {
        let mut grants = self.grants.clone();
        grants.sort();
        grants
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

#[cfg(test)]
mod tests {
    use super::{Granting, Workspace, native};
    use crate::symbol::PackageId;

    /// A declaration that lets its dependency bring native code.
    fn allowing(from: &str, to: &str) -> Granting {
        Granting {
            from: PackageId(from.to_owned()),
            local: to.to_owned(),
            to: PackageId(to.to_owned()),
            abilities: Vec::new(),
            native: true,
        }
    }

    #[test]
    fn native_code_is_brought_only_through_what_may_bring_it() {
        let mut workspace = Workspace::new();
        workspace.hold("http");
        workspace.hold("sqlite");
        let may =
            |workspace: &Workspace, name: &str| native(workspace, &PackageId(name.to_owned()));
        workspace.granting(allowing("http", "sqlite"));
        // `http` may not itself, so it has nothing to pass on.
        assert!(!may(&workspace, "sqlite"));

        workspace.granting(allowing("app", "http"));
        assert!(may(&workspace, "sqlite"));
        assert!(may(&workspace, "app"));
    }
}

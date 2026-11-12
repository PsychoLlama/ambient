//! Symbols: the canonical names of definitions. See `spec/symbols.md`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// One package instance in a workspace.
///
/// TODO: this becomes a workspace member or a locked snapshot once packages
/// have identities (`spec/packages.md`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PackageId(pub String);

/// Where a module sits within its package.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ModulePath(pub Vec<String>);

/// A module: a package instance and a path within it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ModuleId {
    pub package: PackageId,
    pub path: ModulePath,
}

impl ModuleId {
    /// A module from a package name and a dotted path, such as `"util.math"`.
    #[must_use]
    pub fn new(package: &str, path: &str) -> Self {
        Self {
            package: PackageId(package.to_owned()),
            path: ModulePath(path.split('.').map(str::to_owned).collect()),
        }
    }

    #[must_use]
    pub fn symbol(&self, namespace: Namespace, name: &str) -> Symbol {
        Symbol {
            module: self.clone(),
            namespace,
            name: name.to_owned(),
        }
    }

    /// The symbol of a function defined in this module.
    #[must_use]
    pub fn value(&self, name: &str) -> Symbol {
        self.symbol(Namespace::Value, name)
    }

    /// The symbol of a type defined in this module.
    #[must_use]
    pub fn ty(&self, name: &str) -> Symbol {
        self.symbol(Namespace::Type, name)
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.package.0, self.path.0.join("."))
    }
}

/// A module may define the same name once in each namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Namespace {
    Type,
    Value,
    Ability,
}

/// The one canonical name of a definition within a workspace.
///
/// TODO: `name` becomes a path once nested named functions exist.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Symbol {
    pub module: ModuleId,
    pub namespace: Namespace,
    pub name: String,
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.module, self.name)
    }
}

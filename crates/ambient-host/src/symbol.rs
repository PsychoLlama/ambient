//! Symbols: the canonical names of definitions. See `docs/spec/symbols.md`.

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// One package instance in a workspace.
///
/// TODO: this becomes a workspace member or a locked snapshot once packages
/// have identities (`docs/spec/packages.md`).
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
    /// The empty path is the package's library module.
    #[must_use]
    pub fn new(package: &str, path: &str) -> Self {
        let segments = path.split('.').filter(|segment| !segment.is_empty());
        Self {
            package: PackageId(package.to_owned()),
            path: ModulePath(segments.map(str::to_owned).collect()),
        }
    }

    /// The library module of `package`: the one whose definitions are
    /// reached through the package itself, and which lists the modules
    /// other packages may reach. See `docs/spec/packages.md`.
    #[must_use]
    pub fn library(package: &PackageId) -> Self {
        Self {
            package: package.clone(),
            path: ModulePath(Vec::new()),
        }
    }

    #[must_use]
    pub fn symbol(&self, namespace: Namespace, name: &str) -> Symbol {
        Symbol(Arc::new(SymbolData {
            module: self.clone(),
            namespace,
            name: name.to_owned(),
            lambda: Vec::new(),
        }))
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

    /// The symbol of a contract defined in this module.
    #[must_use]
    pub fn contract(&self, name: &str) -> Symbol {
        self.symbol(Namespace::Contract, name)
    }

    /// The symbol of an ability defined in this module.
    #[must_use]
    pub fn ability(&self, name: &str) -> Symbol {
        self.symbol(Namespace::Ability, name)
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
    /// The contracts a record holds a value under besides names. See
    /// `docs/spec/methods.md`.
    Contract,
}

impl Namespace {
    pub const ALL: [Self; 4] = [Self::Type, Self::Value, Self::Ability, Self::Contract];
}

/// The one canonical name of a definition within a workspace.
///
/// A symbol is a shared handle: cloning one is cheap.
///
/// TODO: `name` becomes a path once nested named functions exist.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Symbol(Arc<SymbolData>);

#[derive(Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SymbolData {
    pub module: ModuleId,
    pub namespace: Namespace,
    pub name: String,
    /// Empty for a named definition. For a lambda, its position among the
    /// lambdas of its encloser, at each level of nesting.
    pub lambda: Vec<usize>,
}

impl Symbol {
    /// The lambda at `path` inside this named definition.
    #[must_use]
    pub fn lambda(&self, path: &[usize]) -> Symbol {
        Symbol(Arc::new(SymbolData {
            module: self.module.clone(),
            namespace: self.namespace,
            name: self.name.clone(),
            lambda: path.to_vec(),
        }))
    }

    /// What tells a definition from every other of its package, where
    /// structure cannot: its module path and its name, as one text. With
    /// the package's ID it is the definition's identity. See "Identity"
    /// in `docs/spec/definitions.md`.
    #[must_use]
    pub fn identity(&self) -> String {
        let path = self.module.path.0.iter().map(String::as_str);
        path.chain([self.name.as_str()])
            .collect::<Vec<_>>()
            .join(".")
    }

    /// The named definition this symbol is, or is a lambda inside.
    #[must_use]
    pub fn owner(&self) -> Symbol {
        self.lambda(&[])
    }
}

impl Deref for Symbol {
    type Target = SymbolData;

    fn deref(&self) -> &SymbolData {
        &self.0
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.module, self.name)?;
        for index in &self.lambda {
            write!(f, "#{index}")?;
        }
        Ok(())
    }
}

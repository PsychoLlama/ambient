//! The binaries of a package: the functions of its `main` module that
//! it offers to be run. See "Binaries" in `docs/spec/packages.md`.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use serde::Deserialize;

use super::{Opening, Package, ProjectError};
use crate::surface::Definition;
use crate::symbol::{ModuleId, Namespace, Symbol};
use crate::workspace::Sources;

/// The module a package's binaries are in, and the binary a package
/// that defines that function there has without declaring it.
pub const MAIN: &str = "main";

/// A binary as a manifest declares it, under the name of its function.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bin {
    /// What its executable is called; the binary's own name, if not
    /// said.
    pub name: Option<String>,
    /// The target it is run and built under: a path as a prelude's is,
    /// to a target. The command-line target, if not said.
    pub target: Option<String>,
}

impl Bin {
    /// What is wrong with what is declared under `key`, if anything is.
    pub(super) fn wrong(&self, key: &str) -> Option<&'static str> {
        let file = |name: &String| {
            let mut parts = Path::new(name).components();
            matches!(parts.next(), Some(Component::Normal(_))) && parts.next().is_none()
        };
        match &self.name {
            _ if !named(key) => Some("is not named as a function is"),
            Some(name) if !file(name) => Some("says a `name` that is not the name of a file"),
            _ => None,
        }
    }
}

/// Whether `name` could be the name of a function.
fn named(name: &str) -> bool {
    let mut chars = name.chars();
    let starts = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_');
    starts && chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// A binary of a package that was opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binary {
    /// The function that is run.
    pub entry: Symbol,
    /// What its executable is called.
    pub name: String,
    /// The target it names, if it names one.
    pub target: Option<Symbol>,
}

/// The first two of `named`, each a binary and what its executable is
/// called, that are called the same.
fn rivals<'a>(named: impl IntoIterator<Item = (&'a str, &'a str)>) -> Option<(&'a str, &'a str)> {
    let mut first = BTreeMap::new();
    named
        .into_iter()
        .find_map(|(binary, name)| Some((first.insert(name, binary)?, binary)))
}

impl Opening<'_> {
    /// The binaries of `package`, whose manifest at `path` is
    /// `manifest`: each it declares, and `main` if the module `main`
    /// defines it.
    ///
    /// Every package's are worked out as it is opened, so a manifest
    /// that is wrong is refused whoever opened it.
    pub(super) fn binaries(
        &self,
        package: &str,
        path: &Path,
        manifest: &Package,
    ) -> Result<BTreeMap<String, Binary>, ProjectError> {
        let refuse = |reason: String| ProjectError::Manifest {
            path: path.to_owned(),
            reason,
        };
        let main = ModuleId::new(package, MAIN);
        let defines = |name: &str| {
            let function = |definition: &Definition| {
                definition.name == name && definition.kind.namespace() == Namespace::Value
            };
            let module = self.workspace.module(&main);
            module.is_some_and(|module| module.definitions().iter().any(function))
        };
        let mut declared = manifest.bins.clone();
        if defines(MAIN) {
            declared.entry(MAIN.to_owned()).or_default();
        }
        let mut bins = BTreeMap::new();
        for (key, bin) in declared {
            if !defines(&key) {
                let reason = format!("the binary `{key}` is no function of `src/{MAIN}.ab`");
                return Err(refuse(reason));
            }
            let target = bin.target.as_deref();
            let target = target.map(|target| self.value(package, target, "binary", &key));
            let binary = Binary {
                entry: main.value(&key),
                name: bin.name.unwrap_or_else(|| key.clone()),
                target: target.transpose()?,
            };
            bins.insert(key, binary);
        }
        let named = bins
            .iter()
            .map(|(key, binary)| (key.as_str(), &*binary.name));
        if let Some((one, other)) = rivals(named) {
            let name = &bins[one].name;
            let reason = format!("the binaries `{one}` and `{other}` are both named `{name}`");
            return Err(refuse(reason));
        }
        Ok(bins)
    }
}

#[cfg(test)]
mod tests {
    use super::{named, rivals};

    #[test]
    fn a_binary_is_named_as_a_function_is() {
        assert!(named("main") && named("_deploy2"));
        assert!(!named("") && !named("my-tool") && !named("2nd") && !named("a.b"));
    }

    #[test]
    fn two_binaries_of_one_executable_name_are_rivals() {
        let apart = [("main", "app"), ("deploy", "deploy")];
        let shared = [("deploy", "app"), ("main", "app"), ("other", "o")];
        assert_eq!(rivals(apart), None);
        assert_eq!(rivals(shared), Some(("deploy", "main")));
    }
}

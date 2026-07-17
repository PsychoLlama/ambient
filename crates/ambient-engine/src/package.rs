//! Package representation with lazy module loading.
//!
//! A package is a collection of modules rooted at an `ambient.toml` manifest.
//! Modules are loaded lazily as they're imported.
//!
//! Module paths are **mounted**: every loaded module's [`ModulePath`] begins
//! with the package name (`["foo", "utils"]` for foo's `src/utils.ab`; the
//! package root `main.ab` collapses to the mount itself, `["foo"]`, as a
//! directory module). Mounting is what lets several workspace packages share
//! one registry — see [`crate::module_registry`] — and a standalone package
//! mounts alone so the two layouts never diverge.
//!
//! [`BuildSet`] is the unit a build loads: every workspace member (or the
//! single standalone package), routed by mount.
//!
//! Note: This module provides the package structure, but actual parsing
//! is done by the CLI layer which has access to `ambient-parser`.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;

use crate::ast::Module;
use crate::manifest::Manifest;
use crate::module_path::ModulePath;

/// Errors that can occur when loading a package or module.
#[derive(Debug, Error)]
pub enum LoadError {
    /// The manifest could not be loaded.
    #[error("failed to load manifest: {0}")]
    Manifest(#[from] crate::manifest::ManifestError),

    /// A module file could not be read.
    #[error("module not found: {0}")]
    ModuleNotFound(ModulePath),

    /// A module file could not be parsed.
    #[error("parse error in {0}: {1}")]
    ParseError(ModulePath, String),

    /// IO error reading a file.
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// A parsed module with its source and AST.
#[derive(Debug, Clone)]
pub struct LoadedModule {
    /// The module path.
    pub path: ModulePath,
    /// Whether the module is a *directory module* (backed by a `main.ab`),
    /// which anchors `self`/`super` at its own path. A mounted package root
    /// is one by construction (`foo/main.ab` → `["foo"]`). Registration
    /// must pass this through, or `self::` in a directory module anchors a
    /// level too high.
    pub is_dir_module: bool,
    /// The source code.
    pub source: String,
    /// The parsed AST.
    pub ast: Module,
    /// The module's real on-disk source path, relative to the package `src/`
    /// directory (`shapes/main.ab`), when loaded from disk. `None` for an
    /// in-memory module with no backing file. This is the authority on a
    /// directory module's `<dir>/main.ab` layout — the canonical
    /// [`ModulePath::to_file_path`] reconstruction collapses to the wrong
    /// `<dir>.ab` — so it is both what the loader reads and what
    /// [`crate::module_interface::module_source_path`] records in the snapshot.
    pub source_path: Option<String>,
}

/// A package with lazily-loaded modules.
///
/// Modules are parsed on-demand as they're imported, rather than
/// eagerly parsing all `.ab` files in the source directory.
///
/// The actual parsing is done externally (by the CLI layer with access
/// to ambient-parser) using the `add_module` method.
#[derive(Debug)]
pub struct Package {
    /// Package manifest.
    manifest: Manifest,
    /// The package's mount: its name, interned once. Every loaded module's
    /// path begins with this segment.
    mount: Arc<str>,
    /// Root directory of the package (where ambient.toml is).
    root: PathBuf,
    /// Loaded modules, keyed by module path. A `BTreeMap` (not a `HashMap`) so
    /// [`Self::all_modules`] iterates in a deterministic, source-order-independent
    /// order — build determinism (warm == cold == lazy byte-identity) then holds
    /// by construction rather than relying on every downstream consumer to launder
    /// the iteration order through its own sort.
    modules: BTreeMap<ModulePath, LoadedModule>,
    /// Modules currently being loaded (for cycle detection).
    loading: HashSet<ModulePath>,
}

impl Package {
    /// Create a new package from a manifest and root directory.
    #[must_use]
    pub fn new(manifest: Manifest, root: PathBuf) -> Self {
        let mount = Arc::from(manifest.name.as_str());
        Self {
            manifest,
            mount,
            root,
            modules: BTreeMap::new(),
            loading: HashSet::new(),
        }
    }

    /// Open a package from a directory (finds manifest only, doesn't parse).
    ///
    /// A package needs no `main.ab`: a workspace library member has only the
    /// modules other packages import, and `run` reports a missing entry
    /// function itself.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest cannot be found or loaded.
    pub fn open(path: &std::path::Path) -> Result<Self, LoadError> {
        let (manifest, root) = Manifest::find(path)?;
        Ok(Self::new(manifest, root))
    }

    /// Get the package manifest.
    #[must_use]
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The package's mount segment (its name): the leading segment every
    /// loaded module's path carries.
    #[must_use]
    pub fn mount(&self) -> &Arc<str> {
        &self.mount
    }

    /// The mounted [`ModulePath`] of the package's root module (`main.ab`):
    /// the mount itself, as a directory module.
    #[must_use]
    pub fn root_module(&self) -> ModulePath {
        // A one-segment path is never empty, so this can't be `None`.
        ModulePath::from_segments(vec![Arc::clone(&self.mount)]).unwrap_or_else(ModulePath::root)
    }

    /// Get the package root directory.
    #[must_use]
    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    /// Get the source directory path.
    #[must_use]
    pub fn src_path(&self) -> PathBuf {
        self.root.join(&self.manifest.src_dir)
    }

    /// Get a loaded module by path.
    #[must_use]
    pub fn get_module(&self, path: &ModulePath) -> Option<&LoadedModule> {
        self.modules.get(path)
    }

    /// Get all loaded modules.
    #[must_use]
    pub fn modules(&self) -> &BTreeMap<ModulePath, LoadedModule> {
        &self.modules
    }

    /// Iterate over all loaded modules, in ascending [`ModulePath`] order.
    ///
    /// The order is deterministic and independent of load/insertion order (the
    /// backing store is a `BTreeMap`). Downstream consumers may rely on this for
    /// build determinism and need not re-sort to launder a nondeterministic
    /// iteration order.
    pub fn all_modules(&self) -> impl Iterator<Item = &LoadedModule> {
        self.modules.values()
    }

    /// Iterate mutably over all loaded modules (the resolve pass rewrites
    /// module ASTs in place).
    pub fn all_modules_mut(&mut self) -> impl Iterator<Item = &mut LoadedModule> {
        self.modules.values_mut()
    }

    /// Check if a module is loaded.
    #[must_use]
    pub fn is_loaded(&self, path: &ModulePath) -> bool {
        self.modules.contains_key(path)
    }

    /// Add a parsed module to the package.
    ///
    /// This is called by the loader (CLI layer) after parsing.
    pub fn add_module(&mut self, loaded: LoadedModule) {
        self.loading.remove(&loaded.path);
        self.modules.insert(loaded.path.clone(), loaded);
    }

    /// Read module source code from file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read.
    pub fn read_module_source(&self, path: &ModulePath) -> Result<String, LoadError> {
        let file_path = self.module_file_path(path);

        std::fs::read_to_string(&file_path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                LoadError::ModuleNotFound(path.clone())
            } else {
                LoadError::Io {
                    path: file_path,
                    source,
                }
            }
        })
    }

    /// Mark a module as currently being loaded (for cycle detection).
    ///
    /// # Panics
    ///
    /// Panics if the module is already being loaded (indicates a cycle).
    pub fn mark_loading(&mut self, path: &ModulePath) {
        assert!(
            !self.loading.contains(path),
            "cycle during module loading: {path}"
        );
        self.loading.insert(path.clone());
    }

    /// Get the file path for a module.
    ///
    /// Strips the package mount from a mounted path (`["foo", "utils"]` →
    /// `src/utils.ab`; the mount itself is the root `main.ab`). A path that
    /// doesn't carry the mount maps as-is (package-relative).
    #[must_use]
    pub fn module_file_path(&self, path: &ModulePath) -> PathBuf {
        let segments = path.segments();
        let relative = match segments.split_first() {
            Some((first, rest)) if *first == self.mount => {
                match ModulePath::from_segments(rest.to_vec()) {
                    Some(inner) => inner.to_file_path(),
                    // The mount itself: the package root module.
                    None => PathBuf::from("main.ab"),
                }
            }
            _ => path.to_file_path(),
        };
        self.src_path().join(relative)
    }

    /// The on-disk path to render `module`'s diagnostics against.
    ///
    /// Prefers the real discovered path (a directory module's `<dir>/main.ab`);
    /// falls back to the canonical reconstruction ([`Self::module_file_path`])
    /// only for a module with no recorded on-disk path.
    #[must_use]
    pub fn module_diagnostic_path(&self, module: &LoadedModule, path: &ModulePath) -> PathBuf {
        module.source_path.as_ref().map_or_else(
            || self.module_file_path(path),
            |sp| self.src_path().join(sp),
        )
    }

    /// Check if a module file exists.
    #[must_use]
    pub fn module_exists(&self, path: &ModulePath) -> bool {
        self.module_file_path(path).exists()
    }
}

/// The set of packages one build loads: every workspace member (or the
/// single standalone package), each mounted under its package name.
///
/// Iteration order is deterministic: packages sort by mount, and since every
/// module path begins with its package's mount, chaining the per-package
/// (already sorted) module maps yields ascending [`ModulePath`] order
/// globally — the same invariant [`Package::all_modules`] documents.
#[derive(Debug)]
pub struct BuildSet {
    packages: Vec<Package>,
}

impl BuildSet {
    /// Build a set from loaded packages, sorting by mount.
    #[must_use]
    pub fn new(mut packages: Vec<Package>) -> Self {
        packages.sort_by(|a, b| a.mount().cmp(b.mount()));
        Self { packages }
    }

    /// The member packages, ascending by mount.
    #[must_use]
    pub fn packages(&self) -> &[Package] {
        &self.packages
    }

    /// Mutable access for the load phase.
    pub fn packages_mut(&mut self) -> &mut [Package] {
        &mut self.packages
    }

    /// The package mounted at `path`'s leading segment.
    #[must_use]
    pub fn package_of(&self, path: &ModulePath) -> Option<&Package> {
        let first = path.segments().first()?;
        self.packages.iter().find(|p| p.mount() == first)
    }

    /// The package named `name`.
    #[must_use]
    pub fn package_named(&self, name: &str) -> Option<&Package> {
        self.packages.iter().find(|p| p.mount().as_ref() == name)
    }

    /// Every loaded module across the set, ascending by [`ModulePath`].
    pub fn all_modules(&self) -> impl Iterator<Item = &LoadedModule> {
        self.packages.iter().flat_map(Package::all_modules)
    }

    /// Mutable iteration (the resolve pass rewrites ASTs in place).
    pub fn all_modules_mut(&mut self) -> impl Iterator<Item = &mut LoadedModule> {
        self.packages.iter_mut().flat_map(Package::all_modules_mut)
    }

    /// A loaded module by its mounted path, routed to the owning package.
    #[must_use]
    pub fn get_module(&self, path: &ModulePath) -> Option<&LoadedModule> {
        self.package_of(path)?.get_module(path)
    }

    /// The on-disk path to render `module`'s diagnostics against. Falls back
    /// to the canonical reconstruction when the owning package is unknown.
    #[must_use]
    pub fn module_diagnostic_path(&self, module: &LoadedModule, path: &ModulePath) -> PathBuf {
        self.package_of(path).map_or_else(
            || path.to_file_path(),
            |pkg| pkg.module_diagnostic_path(module, path),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_package(files: &[(&str, &str)]) -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("create temp dir");
        let root = dir.path().to_path_buf();

        // Create manifest
        fs::write(
            root.join("ambient.toml"),
            r#"[package]
name = "test_pkg"
version = "0.1.0"

[build]
src = "src"
"#,
        )
        .expect("write manifest");

        // Create src directory
        let src = root.join("src");
        fs::create_dir_all(&src).expect("create src dir");

        // Create files
        for (path, content) in files {
            let file_path = src.join(path);
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent).expect("create parent dir");
            }
            fs::write(&file_path, content).expect("write file");
        }

        (dir, root)
    }

    #[test]
    fn test_open_package() {
        let (_dir, root) = create_test_package(&[("main.ab", "fn run(): number { 42 }")]);

        let pkg = Package::open(&root).expect("open package");

        assert_eq!(pkg.manifest().name, "test_pkg");
        // Modules are not loaded on open
        assert!(!pkg.is_loaded(&ModulePath::root()));
    }

    #[test]
    fn test_read_module_source() {
        let (_dir, root) = create_test_package(&[
            ("main.ab", "fn run(): number { 42 }"),
            ("utils.ab", "fn helper(): number { 1 }"),
        ]);

        let pkg = Package::open(&root).expect("open package");

        let utils_path = ModulePath::from_str_segments(&["utils"]).unwrap();
        let source = pkg.read_module_source(&utils_path).expect("read source");
        assert!(source.contains("fn helper"));
    }

    #[test]
    fn test_read_nested_module_source() {
        let (_dir, root) = create_test_package(&[
            ("main.ab", "fn run(): number { 42 }"),
            ("utils/format.ab", "fn fmt(): string { \"\" }"),
        ]);

        let pkg = Package::open(&root).expect("open package");

        let path = ModulePath::from_str_segments(&["utils", "format"]).unwrap();
        let source = pkg.read_module_source(&path).expect("read source");
        assert!(source.contains("fn fmt"));
    }

    #[test]
    fn test_read_missing_module() {
        let (_dir, root) = create_test_package(&[("main.ab", "fn run(): number { 42 }")]);

        let pkg = Package::open(&root).expect("open package");

        let path = ModulePath::from_str_segments(&["nonexistent"]).unwrap();
        let err = pkg.read_module_source(&path).unwrap_err();

        assert!(matches!(err, LoadError::ModuleNotFound(_)));
    }

    /// A package needs no `main.ab`: a workspace library member has only
    /// the modules other packages import.
    #[test]
    fn test_open_without_entry_point() {
        let dir = TempDir::new().expect("create temp dir");
        let root = dir.path().to_path_buf();

        fs::write(
            root.join("ambient.toml"),
            r#"[package]
name = "test"
version = "0.1.0"
"#,
        )
        .expect("write manifest");

        fs::create_dir_all(root.join("src")).expect("create src dir");
        // No main.ab

        let pkg = Package::open(&root).expect("open package");
        assert_eq!(pkg.manifest().name, "test");
    }

    /// Mounted module paths map back to package-relative files, and the
    /// mount itself is the root `main.ab`.
    #[test]
    fn test_mounted_module_file_path() {
        let (_dir, root) = create_test_package(&[("main.ab", "fn run(): number { 42 }")]);
        let pkg = Package::open(&root).expect("open package");

        assert_eq!(pkg.mount().as_ref(), "test_pkg");
        let mounted = ModulePath::from_str_segments(&["test_pkg", "utils", "format"]).unwrap();
        assert_eq!(
            pkg.module_file_path(&mounted),
            pkg.src_path().join("utils/format.ab")
        );
        assert_eq!(
            pkg.module_file_path(&pkg.root_module()),
            pkg.src_path().join("main.ab")
        );
    }

    #[test]
    fn test_module_exists() {
        let (_dir, root) = create_test_package(&[
            ("main.ab", "fn run(): number { 42 }"),
            ("utils.ab", "fn helper(): number { 1 }"),
        ]);

        let pkg = Package::open(&root).expect("open package");

        let utils = ModulePath::from_str_segments(&["utils"]).unwrap();
        let missing = ModulePath::from_str_segments(&["missing"]).unwrap();

        assert!(pkg.module_exists(&utils));
        assert!(!pkg.module_exists(&missing));
    }

    #[test]
    fn test_add_module() {
        let (_dir, root) = create_test_package(&[("main.ab", "fn run(): number { 42 }")]);

        let mut pkg = Package::open(&root).expect("open package");

        let path = ModulePath::root();
        assert!(!pkg.is_loaded(&path));

        // Add a dummy module (in real usage, this would be parsed by ambient-parser)
        let loaded = LoadedModule {
            path: path.clone(),
            is_dir_module: false,
            source: "fn run(): number { 42 }".to_string(),
            ast: Module {
                name: "main".into(),
                doc: None,
                items: vec![],
            },
            source_path: None,
        };
        pkg.add_module(loaded);

        assert!(pkg.is_loaded(&path));
    }
}

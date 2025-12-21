//! Package representation with lazy module loading.
//!
//! A package is a collection of modules rooted at an `ambient.toml` manifest.
//! Modules are loaded lazily as they're imported.
//!
//! Note: This module provides the package structure, but actual parsing
//! is done by the CLI layer which has access to `ambient-parser`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

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

    /// Entry point main.ab not found.
    #[error("entry point not found: {0}/main.ab")]
    NoEntryPoint(PathBuf),
}

/// A parsed module with its source and AST.
#[derive(Debug, Clone)]
pub struct LoadedModule {
    /// The module path.
    pub path: ModulePath,
    /// The source code.
    pub source: String,
    /// The parsed AST.
    pub ast: Module,
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
    /// Root directory of the package (where ambient.toml is).
    root: PathBuf,
    /// Loaded modules, keyed by module path.
    modules: HashMap<ModulePath, LoadedModule>,
    /// Modules currently being loaded (for cycle detection).
    loading: HashSet<ModulePath>,
}

impl Package {
    /// Create a new package from a manifest and root directory.
    #[must_use]
    pub fn new(manifest: Manifest, root: PathBuf) -> Self {
        Self {
            manifest,
            root,
            modules: HashMap::new(),
            loading: HashSet::new(),
        }
    }

    /// Open a package from a directory (finds manifest only, doesn't parse).
    ///
    /// This verifies the manifest exists and the entry point file exists,
    /// but does not parse any modules.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest cannot be found or loaded,
    /// or if main.ab doesn't exist.
    pub fn open(path: &std::path::Path) -> Result<Self, LoadError> {
        let (manifest, root) = Manifest::find(path)?;
        let pkg = Self::new(manifest, root);

        // Verify entry point exists
        let main_path = pkg.src_path().join("main.ab");
        if !main_path.exists() {
            return Err(LoadError::NoEntryPoint(pkg.src_path()));
        }

        Ok(pkg)
    }

    /// Get the package manifest.
    #[must_use]
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
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
    pub fn modules(&self) -> &HashMap<ModulePath, LoadedModule> {
        &self.modules
    }

    /// Iterate over all loaded modules.
    pub fn all_modules(&self) -> impl Iterator<Item = &LoadedModule> {
        self.modules.values()
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
    #[must_use]
    pub fn module_file_path(&self, path: &ModulePath) -> PathBuf {
        self.src_path().join(path.to_file_path())
    }

    /// Check if a module file exists.
    #[must_use]
    pub fn module_exists(&self, path: &ModulePath) -> bool {
        self.module_file_path(path).exists()
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

    #[test]
    fn test_no_entry_point() {
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

        let err = Package::open(&root).unwrap_err();
        assert!(matches!(err, LoadError::NoEntryPoint(_)));
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
            source: "fn run(): number { 42 }".to_string(),
            ast: Module {
                name: "main".into(),
                doc: None,
                items: vec![],
            },
        };
        pkg.add_module(loaded);

        assert!(pkg.is_loaded(&path));
    }
}

//! Package discovery and module registry building for the LSP.
//!
//! This module handles discovering the package root from a file URI,
//! loading sibling modules, and building a `ModuleRegistry` for cross-module
//! type checking.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ambient_engine::ast::Module;
use ambient_engine::manifest::Manifest;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;

use lsp_types::Uri;

/// Information about a discovered package.
#[derive(Debug)]
pub struct PackageInfo {
    /// The package root directory (where ambient.toml is).
    pub root: PathBuf,
    /// The source directory.
    pub src_dir: PathBuf,
    /// Parsed modules in the package, keyed by module path.
    pub modules: HashMap<String, ParsedModule>,
}

/// A parsed module with its source and AST.
#[derive(Debug, Clone)]
pub struct ParsedModule {
    /// The module path.
    pub path: ModulePath,
    /// The source code.
    pub source: String,
    /// The parsed AST.
    pub ast: Module,
}

impl PackageInfo {
    /// Discover the package containing a file.
    ///
    /// Walks up the directory tree looking for ambient.toml.
    pub fn discover(file_uri: &Uri) -> Option<Self> {
        let file_path = uri_to_path(file_uri)?;
        let file_dir = file_path.parent()?;

        // Walk up looking for ambient.toml
        let mut current = file_dir;
        loop {
            let manifest_path = current.join("ambient.toml");
            if manifest_path.exists() {
                // Found the package root
                let manifest = Manifest::from_file(&manifest_path).ok()?;
                let src_dir = current.join(&manifest.src_dir);
                return Some(Self {
                    root: current.to_path_buf(),
                    src_dir,
                    modules: HashMap::new(),
                });
            }

            current = current.parent()?;
        }
    }

    /// Get the module path for a file URI within this package.
    pub fn uri_to_module_path(&self, uri: &Uri) -> Option<ModulePath> {
        let file_path = uri_to_path(uri)?;
        let relative = file_path.strip_prefix(&self.src_dir).ok()?;

        // Convert path to module segments
        let mut segments = Vec::new();
        for component in relative.components() {
            if let std::path::Component::Normal(s) = component {
                let name = s.to_str()?;
                // Remove .ab extension from the last component
                let name = name.strip_suffix(".ab").unwrap_or(name);
                // Skip "main" as it's the root module
                if name != "main" || !segments.is_empty() {
                    segments.push(Arc::from(name));
                }
            }
        }

        // If the only segment was "main", it's the root module
        if segments.is_empty() {
            Some(ModulePath::root())
        } else {
            ModulePath::from_segments(segments)
        }
    }

    /// Discover and parse all .ab files in the source directory.
    pub fn discover_modules(&mut self) {
        if let Ok(entries) = discover_ab_files(&self.src_dir) {
            for entry in entries {
                if let Some((path, source, ast)) = self.load_module_file(&entry) {
                    let key = path.to_string();
                    self.modules.insert(key, ParsedModule { path, source, ast });
                }
            }
        }
    }

    /// Load and parse a single module file.
    fn load_module_file(&self, file_path: &Path) -> Option<(ModulePath, String, Module)> {
        let relative = file_path.strip_prefix(&self.src_dir).ok()?;

        // Convert path to module segments
        let mut segments = Vec::new();
        for component in relative.components() {
            if let std::path::Component::Normal(s) = component {
                let name = s.to_str()?;
                let name = name.strip_suffix(".ab").unwrap_or(name);
                if name != "main" || !segments.is_empty() {
                    segments.push(Arc::from(name));
                }
            }
        }

        let module_path = if segments.is_empty() {
            ModulePath::root()
        } else {
            ModulePath::from_segments(segments)?
        };

        let source = std::fs::read_to_string(file_path).ok()?;
        let ast = ambient_parser::parse(&source).ok()?;

        Some((module_path, source, ast))
    }

    /// Build a module registry from discovered modules.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn build_registry(&self) -> ModuleRegistry {
        let mut registry = ModuleRegistry::new();

        for module in self.modules.values() {
            registry.register(&module.path, Arc::new(module.ast.clone()));
        }

        registry
    }

    /// Update a module in the package (called when file changes).
    pub fn update_module(&mut self, uri: &Uri, source: &str, ast: Module) {
        if let Some(path) = self.uri_to_module_path(uri) {
            let key = path.to_string();
            self.modules.insert(
                key,
                ParsedModule {
                    path,
                    source: source.to_string(),
                    ast,
                },
            );
        }
    }
}

/// Recursively discover all .ab files in a directory.
fn discover_ab_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    if !dir.is_dir() {
        return Ok(files);
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            files.extend(discover_ab_files(&path)?);
        } else if path.extension().is_some_and(|ext| ext == "ab") {
            files.push(path);
        }
    }

    Ok(files)
}

/// Convert a file:// URI to a path.
fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let uri_str = uri.as_str();
    if !uri_str.starts_with("file://") {
        return None;
    }

    let path_str = uri_str.strip_prefix("file://")?;
    Some(PathBuf::from(percent_decode(path_str)))
}

/// Decode percent-encoded characters in a URI path.
fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    result.push(byte as char);
                    continue;
                }
            }
            result.push('%');
            result.push_str(&hex);
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_package() -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("create temp dir");
        let root = dir.path().to_path_buf();

        // Create manifest
        fs::write(
            root.join("ambient.toml"),
            r#"[package]
name = "test"
version = "0.1.0"

[build]
src = "src"
"#,
        )
        .expect("write manifest");

        // Create source files
        let src = root.join("src");
        fs::create_dir_all(&src).expect("create src dir");
        fs::write(src.join("main.ab"), "fn main(): number { 42 }").expect("write main");
        fs::write(src.join("utils.ab"), "pub fn helper(): number { 1 }").expect("write utils");

        (dir, root)
    }

    #[test]
    fn test_discover_package() {
        let (_dir, root) = create_test_package();
        let main_uri: Uri = format!("file://{}/src/main.ab", root.display())
            .parse()
            .expect("valid uri");

        let pkg = PackageInfo::discover(&main_uri);
        assert!(pkg.is_some());

        let pkg = pkg.expect("package");
        assert_eq!(pkg.root, root);
    }

    #[test]
    fn test_uri_to_module_path() {
        let (_dir, root) = create_test_package();
        let main_uri: Uri = format!("file://{}/src/main.ab", root.display())
            .parse()
            .expect("valid uri");

        let pkg = PackageInfo::discover(&main_uri).expect("package");

        // main.ab -> root module
        let path = pkg.uri_to_module_path(&main_uri);
        assert!(path.is_some());
        assert_eq!(path.expect("path"), ModulePath::root());

        // utils.ab -> utils module
        let utils_uri: Uri = format!("file://{}/src/utils.ab", root.display())
            .parse()
            .expect("valid uri");
        let path = pkg.uri_to_module_path(&utils_uri);
        assert!(path.is_some());
        assert_eq!(path.expect("path").to_string(), "utils");
    }

    #[test]
    fn test_discover_modules() {
        let (_dir, root) = create_test_package();
        let main_uri: Uri = format!("file://{}/src/main.ab", root.display())
            .parse()
            .expect("valid uri");

        let mut pkg = PackageInfo::discover(&main_uri).expect("package");
        pkg.discover_modules();

        assert_eq!(pkg.modules.len(), 2); // main and utils
        assert!(pkg.modules.contains_key("main")); // root module
        assert!(pkg.modules.contains_key("utils"));
    }

    #[test]
    fn test_build_registry() {
        let (_dir, root) = create_test_package();
        let main_uri: Uri = format!("file://{}/src/main.ab", root.display())
            .parse()
            .expect("valid uri");

        let mut pkg = PackageInfo::discover(&main_uri).expect("package");
        pkg.discover_modules();

        let registry = pkg.build_registry();
        assert!(registry.contains(&ModulePath::root()));
    }
}

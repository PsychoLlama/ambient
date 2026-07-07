//! Package discovery and registry building.
//!
//! One implementation of "load an Ambient package for analysis", shared by
//! `ambient check` and the language server. Modules parse with error
//! recovery, so a file mid-edit still contributes its parseable items to
//! cross-module resolution instead of vanishing from the package.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ambient_engine::ast::Module;
use ambient_engine::manifest::Manifest;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;
use ambient_parser::ParseError;

use crate::AnalysisResult;

/// A package opened for analysis: manifest info plus every parsed module.
#[derive(Debug)]
pub struct AnalysisPackage {
    /// The package root directory (where ambient.toml is).
    pub root: PathBuf,
    /// The source directory.
    pub src_dir: PathBuf,
    /// The package name (`[package].name`). This is the workspace scope
    /// every user item's `Fqn` is minted under, so it must match what
    /// `ambient_engine::build::build_package` uses — otherwise a consumer
    /// that links analysis output against a build (the REPL) sees
    /// mismatched identities. Empty for an in-memory session with no
    /// manifest.
    pub package_name: String,
    /// Parsed modules, keyed by module path string.
    pub modules: HashMap<String, ParsedModule>,
    /// Host abilities configured in `[host].abilities`.
    pub host_abilities: Vec<String>,
}

/// A parsed module with its source and (possibly partial) AST.
#[derive(Debug, Clone)]
pub struct ParsedModule {
    /// The module path.
    pub path: ModulePath,
    /// The source code.
    pub source: String,
    /// The parsed AST — partial when the file has syntax errors.
    pub ast: Module,
    /// Parse errors, if any.
    pub parse_errors: Vec<ParseError>,
}

impl AnalysisPackage {
    /// Discover the package containing `file`, walking up to ambient.toml.
    #[must_use]
    pub fn discover(file: &Path) -> Option<Self> {
        let mut current = file.parent()?;
        loop {
            let manifest_path = current.join("ambient.toml");
            if manifest_path.exists() {
                let manifest = Manifest::from_file(&manifest_path).ok()?;
                return Some(Self {
                    root: current.to_path_buf(),
                    src_dir: current.join(&manifest.src_dir),
                    package_name: manifest.name,
                    modules: HashMap::new(),
                    host_abilities: manifest.host_abilities,
                });
            }
            current = current.parent()?;
        }
    }

    /// Create an empty, in-memory package with no modules loaded from disk.
    ///
    /// Used by the REPL, which accumulates a single synthetic `repl` module
    /// in memory and re-checks it each turn. `root`/`src_dir` are notional —
    /// nothing is read from them — but `insert_module` and `build_registry`
    /// work exactly as they do for an on-disk package.
    #[must_use]
    pub fn empty(root: PathBuf, src_dir: PathBuf) -> Self {
        Self {
            root,
            src_dir,
            package_name: String::new(),
            modules: HashMap::new(),
            host_abilities: Vec::new(),
        }
    }

    /// Open the package rooted at `root` (must contain ambient.toml).
    pub fn open(root: &Path) -> Result<Self, String> {
        let manifest_path = root.join("ambient.toml");
        let manifest = Manifest::from_file(&manifest_path)
            .map_err(|e| format!("failed to read {}: {e}", manifest_path.display()))?;
        let mut package = Self {
            root: root.to_path_buf(),
            src_dir: root.join(&manifest.src_dir),
            package_name: manifest.name,
            modules: HashMap::new(),
            host_abilities: manifest.host_abilities,
        };
        package.load_modules();
        Ok(package)
    }

    /// The module path for a source file inside this package.
    #[must_use]
    pub fn module_path_for(&self, file: &Path) -> Option<ModulePath> {
        let relative = file.strip_prefix(&self.src_dir).ok()?;
        ModulePath::from_relative_file_path(relative)
    }

    /// The source file for a module path inside this package.
    #[must_use]
    pub fn file_for_module(&self, path: &ModulePath) -> PathBuf {
        self.src_dir.join(path.to_file_path())
    }

    /// Discover and parse every `.ab` file under the source directory.
    ///
    /// Files with syntax errors still register with their parseable items,
    /// so the rest of the package resolves imports against them.
    pub fn load_modules(&mut self) {
        for file in discover_ab_files(&self.src_dir) {
            let Some(module_path) = self.module_path_for(&file) else {
                continue;
            };
            let Ok(source) = std::fs::read_to_string(&file) else {
                continue;
            };
            self.insert_module(module_path, source);
        }
    }

    /// Insert or replace a module from source (e.g. an in-editor buffer).
    pub fn insert_module(&mut self, path: ModulePath, source: String) {
        let recovered = ambient_parser::parse_recovering(&source);
        let parse_errors = recovered.errors;
        let mut ast = recovered.module;
        ast.name = path
            .segments()
            .last()
            .cloned()
            .unwrap_or_else(|| Arc::from("main"));
        self.modules.insert(
            path.to_string(),
            ParsedModule {
                path,
                source,
                ast,
                parse_errors,
            },
        );
    }

    /// Build the module registry: core + platform declaration modules plus
    /// every package module. This is the same registry shape the engine's
    /// build pipeline checks against.
    ///
    /// Two passes, mirroring the engine build pipeline (`build.rs`): register
    /// the raw ASTs so resolution can see every module (imports may point
    /// anywhere in the package), then canonicalize each module and *replace*
    /// its raw AST with the resolved one. The replacement matters because
    /// cross-module signature hydration reads a foreign item's signature
    /// straight from the registry — if that signature still spells an ability
    /// bare (e.g. `with Stdio`), it gets re-resolved against the *importing*
    /// module's scope and spuriously fails whenever that module doesn't also
    /// import the ability. Registering the resolved AST (`with platform.Stdio`)
    /// keeps `ambient check`/LSP in step with `ambient run` on multi-module
    /// packages.
    #[must_use]
    pub fn build_registry(&self) -> ModuleRegistry {
        let mut registry = crate::core_platform_registry();
        // Mint user items' `Fqn`s under the package's workspace scope — the
        // same scope `build_package` uses — so the resolve pass below stamps
        // identities that match a compiled build. This keeps `ambient
        // check`/LSP identity-consistent with `ambient run`, and lets the
        // REPL link its session module against a `build_package` base.
        if !self.package_name.is_empty() {
            registry.set_workspace_name(self.package_name.as_str());
        }
        for module in self.modules.values() {
            registry.register(&module.path, Arc::new(module.ast.clone()));
        }
        for module in self.modules.values() {
            let mut ast = module.ast.clone();
            let _ = ambient_engine::resolve::resolve_module(&mut ast, &module.path, &registry);
            registry.register(&module.path, Arc::new(ast));
        }
        registry
    }

    /// Analyze every module in the package against a shared registry.
    ///
    /// Returns results keyed by module path string, in no particular
    /// order. This is what `ambient check <package>` reports, and what the
    /// LSP reports across open files — one computation, two renderers.
    #[must_use]
    pub fn analyze_all(&self) -> HashMap<String, AnalysisResult> {
        let registry = self.build_registry();
        self.modules
            .iter()
            .map(|(key, module)| {
                let result = crate::analyze_with_registry(
                    &module.source,
                    Some(&module.path),
                    Some(&registry),
                );
                (key.clone(), result)
            })
            .collect()
    }
}

/// Recursively discover all `.ab` files under a directory, sorted for
/// deterministic ordering.
fn discover_ab_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_ab_files(dir, &mut files);
    files.sort();
    files
}

fn collect_ab_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_ab_files(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "ab") {
            files.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_package() -> TempDir {
        let dir = TempDir::new().expect("create temp dir");
        let root = dir.path();

        fs::write(
            root.join("ambient.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n\n[build]\nsrc = \"src\"\n",
        )
        .expect("write manifest");

        let src = root.join("src");
        fs::create_dir_all(&src).expect("create src dir");
        fs::write(
            src.join("main.ab"),
            "use pkg::utils::helper;\npub fn run(): Number { helper() }",
        )
        .expect("write main");
        fs::write(src.join("utils.ab"), "pub fn helper(): Number { 1 }").expect("write utils");

        dir
    }

    #[test]
    fn open_loads_all_modules() {
        let dir = create_test_package();
        let package = AnalysisPackage::open(dir.path()).expect("open package");
        assert_eq!(package.modules.len(), 2);
        assert!(package.modules.contains_key("main"));
        assert!(package.modules.contains_key("utils"));
    }

    #[test]
    fn analyze_all_is_clean_for_valid_package() {
        let dir = create_test_package();
        let package = AnalysisPackage::open(dir.path()).expect("open package");
        let results = package.analyze_all();
        for (path, result) in &results {
            assert!(
                result.diagnostics().is_empty(),
                "unexpected diagnostics in {path}: {:?}",
                result.diagnostics()
            );
        }
    }

    #[test]
    fn broken_module_still_contributes_parseable_items() {
        let dir = create_test_package();
        // utils has one broken and one good function; main imports the
        // good one and must still resolve.
        fs::write(
            dir.path().join("src/utils.ab"),
            "fn broken(\n\npub fn helper(): Number { 1 }",
        )
        .expect("rewrite utils");

        let package = AnalysisPackage::open(dir.path()).expect("open package");
        let results = package.analyze_all();

        let utils = &results["utils"];
        assert!(!utils.parse_errors.is_empty());

        let main = &results["main"];
        assert!(
            main.diagnostics().is_empty(),
            "main should resolve helper from the partial module: {:?}",
            main.diagnostics()
        );
    }

    #[test]
    fn multi_module_use_imported_ability_checks_clean() {
        // Regression: `ambient check`/LSP must match `ambient run` on a
        // multi-module package whose entry point imports a platform ability
        // via `use`. The bug was that cross-module signature hydration read
        // `run`'s foreign `with Stdio` signature raw (bare `Stdio`) and
        // re-resolved it against the *other* module's scope, which doesn't
        // import Stdio — a false "not in scope" error. The sibling here is
        // deliberately empty of abilities: adding it must not change whether
        // `main` resolves its own import.
        let dir = TempDir::new().expect("create temp dir");
        let root = dir.path();
        fs::write(
            root.join("ambient.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n\n[build]\nsrc = \"src\"\n",
        )
        .expect("write manifest");
        let src = root.join("src");
        fs::create_dir_all(&src).expect("create src dir");
        fs::write(
            src.join("main.ab"),
            "use core::system::Stdio;\n\npub fn run(): () with Stdio {\n    Stdio::out!(\"hi\")\n}\n",
        )
        .expect("write main");
        fs::write(src.join("sibling.ab"), "pub fn noop(): Number { 1 }\n").expect("write sibling");

        let package = AnalysisPackage::open(root).expect("open package");
        let results = package.analyze_all();
        for (path, result) in &results {
            assert!(
                result.diagnostics().is_empty(),
                "unexpected diagnostics in {path}: {:?}",
                result.diagnostics()
            );
        }
    }

    #[test]
    fn missing_import_is_reported() {
        let dir = create_test_package();
        fs::write(
            dir.path().join("src/main.ab"),
            "use pkg::utils::nonexistent;\npub fn run(): Number { 1 }",
        )
        .expect("rewrite main");

        let package = AnalysisPackage::open(dir.path()).expect("open package");
        let results = package.analyze_all();
        assert!(!results["main"].diagnostics().is_empty());
    }
}

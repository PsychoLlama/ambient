//! Package building and symbol database population.
//!
//! This module provides functionality for compiling Ambient packages
//! and populating the symbol database with the compiled results.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use crate::ast::{ItemKind, Module, UsePrefix};
use crate::compiler::CompiledModule;
use crate::module_path::{ImportPrefix, ModulePath};
use crate::module_registry::{ModuleRegistry, ResolvedImport};
use crate::package::{LoadedModule, Package};
use crate::symbol_db::SymbolDb;

/// Progress callback for reporting build progress.
///
/// Called with (module name, current, total) for each module.
pub type ProgressCallback<'a> = &'a dyn Fn(&str, usize, usize);

/// Parse function type for parsing source code into an AST.
pub type ParseFn = fn(&str) -> Result<Module, String>;

/// Result of building a package.
pub struct BuildResult {
    /// The compiled module containing all functions.
    pub compiled: CompiledModule,
    /// Number of modules compiled.
    pub module_count: usize,
    /// Package name.
    pub package_name: String,
}

/// Error during package building.
#[derive(Debug)]
pub enum BuildError {
    /// Failed to open the package.
    PackageOpen(String),
    /// Failed to parse a module.
    Parse { module: String, error: String },
    /// Type checking failed.
    TypeCheck { module: String, errors: Vec<String> },
    /// Compilation failed.
    Compile { module: String, error: String },
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PackageOpen(msg) => write!(f, "failed to open package: {msg}"),
            Self::Parse { module, error } => write!(f, "parse error in {module}: {error}"),
            Self::TypeCheck { module, errors } => {
                write!(f, "type errors in {module}: {}", errors.join(", "))
            }
            Self::Compile { module, error } => write!(f, "compile error in {module}: {error}"),
        }
    }
}

impl std::error::Error for BuildError {}

/// Build an Ambient package and populate the symbol database.
///
/// This function compiles all modules in topological order and populates
/// the symbol database with function definitions and dependencies.
///
/// # Arguments
///
/// * `path` - Path to the package root directory (containing `ambient.toml`)
/// * `parse` - Function to parse source code into an AST
/// * `progress` - Optional callback for reporting progress
///
/// # Errors
///
/// Returns an error if:
/// - The package cannot be opened (missing manifest, invalid format)
/// - A module fails to parse
/// - Type checking fails
/// - Compilation fails
#[allow(clippy::arc_with_non_send_sync)]
pub fn build_package(
    path: &Path,
    parse: ParseFn,
    progress: Option<ProgressCallback<'_>>,
) -> Result<BuildResult, BuildError> {
    // Open package (validates manifest and entry point).
    let mut pkg = Package::open(path).map_err(|e| BuildError::PackageOpen(e.to_string()))?;

    let package_name = pkg.manifest().name.clone();

    // Load the main module and all its dependencies.
    let main_path = ModulePath::root();
    load_module_with_deps(&mut pkg, &main_path, parse)?;

    // Build module registry with all loaded modules.
    let mut registry = ModuleRegistry::new();
    for module in pkg.all_modules() {
        registry.register(&module.path, Arc::new(module.ast.clone()));
    }

    // Get modules in topological order (dependencies first).
    let module_order = get_compilation_order(&pkg, &main_path);
    let total_modules = module_order.len();

    // Open symbol database in build directory (create if needed).
    let build_dir = path.join("build");
    fs::create_dir_all(&build_dir).ok();
    let db_path = build_dir.join("symbols.db");
    let mut symbol_db = SymbolDb::open(&db_path).ok();

    // Compile modules in dependency order, tracking function hashes.
    let mut all_compiled = CompiledModule::new();
    let mut module_function_hashes: HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>> =
        HashMap::new();

    for (idx, module_path) in module_order.iter().enumerate() {
        let module = pkg
            .get_module(module_path)
            .ok_or_else(|| BuildError::PackageOpen(format!("module not found: {module_path}")))?;
        let file_path = pkg.module_file_path(module_path);

        // Report progress
        if let Some(ref cb) = progress {
            cb(&module_path.to_string(), idx + 1, total_modules);
        }

        // Build imported function hashes from already-compiled dependencies.
        let imported_hashes =
            build_imported_hashes_from_compiled(module_path, &registry, &module_function_hashes);

        let compiled = compile_loaded_module_with_registry(
            module,
            &file_path,
            module_path,
            &registry,
            imported_hashes,
        )?;

        // Populate symbol database with compiled module.
        if let Some(ref mut db) = symbol_db {
            let visibility = extract_function_visibility(&module.ast);
            let module_path_str = module_path.to_string();

            // Clean up old symbols for this module before repopulating.
            let _ = db.remove_module(&module_path_str);

            // Populate with new symbols.
            let _ =
                db.populate_from_module(&compiled, &package_name, &module_path_str, &visibility);
        }

        // Record this module's function hashes for dependents.
        let mut func_hashes = HashMap::new();
        for (name, hash) in &compiled.function_names {
            func_hashes.insert(Arc::clone(name), *hash);
        }
        module_function_hashes.insert(module_path.clone(), func_hashes);

        // Merge into the final module.
        all_compiled.merge(&compiled);
    }

    Ok(BuildResult {
        compiled: all_compiled,
        module_count: total_modules,
        package_name,
    })
}

/// Extract function visibility from a module AST.
fn extract_function_visibility(module: &crate::ast::Module) -> HashMap<Arc<str>, bool> {
    let mut visibility = HashMap::new();
    for item in &module.items {
        if let ItemKind::Function(func) = &item.kind {
            visibility.insert(Arc::clone(&func.name), func.is_public);
        }
    }
    visibility
}

/// Get modules in topological order (dependencies first).
#[allow(clippy::items_after_statements)]
fn get_compilation_order(pkg: &Package, main_path: &ModulePath) -> Vec<ModulePath> {
    let mut order = Vec::new();
    let mut visited = HashSet::new();

    fn visit(
        pkg: &Package,
        path: &ModulePath,
        visited: &mut HashSet<String>,
        order: &mut Vec<ModulePath>,
    ) {
        let key = path.to_string();
        if visited.contains(&key) {
            return;
        }
        visited.insert(key);

        // Visit dependencies first.
        if let Some(module) = pkg.get_module(path) {
            let deps = extract_dependencies(&module.ast, path);
            for dep in deps {
                visit(pkg, &dep, visited, order);
            }
        }

        // Add this module after its dependencies.
        order.push(path.clone());
    }

    visit(pkg, main_path, &mut visited, &mut order);
    order
}

/// Build imported function hashes from already-compiled modules.
fn build_imported_hashes_from_compiled(
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    compiled_hashes: &HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>>,
) -> HashMap<Arc<str>, blake3::Hash> {
    let mut hashes = HashMap::new();

    if let Ok(imports) = registry.resolve_imports(module_path) {
        for (local_name, resolved) in imports {
            if let ResolvedImport::Symbol {
                from_module,
                export_kind: _,
            } = resolved
            {
                if let Some(module_hashes) = compiled_hashes.get(&from_module) {
                    if let Some(hash) = module_hashes.get(&local_name) {
                        hashes.insert(local_name, *hash);
                    }
                }
            }
        }
    }

    hashes
}

/// Load a module and all its dependencies recursively.
fn load_module_with_deps(
    pkg: &mut Package,
    path: &ModulePath,
    parse: ParseFn,
) -> Result<(), BuildError> {
    if pkg.is_loaded(path) {
        return Ok(());
    }

    let loaded = load_module(pkg, path, parse)?;
    let deps = extract_dependencies(&loaded.ast, path);
    pkg.add_module(loaded);

    for dep_path in deps {
        load_module_with_deps(pkg, &dep_path, parse)?;
    }

    Ok(())
}

/// Extract module dependencies from use statements.
fn extract_dependencies(module: &crate::ast::Module, current_path: &ModulePath) -> Vec<ModulePath> {
    let mut deps = Vec::new();
    let mut seen = HashSet::new();

    for item in &module.items {
        if let ItemKind::Use(use_def) = &item.kind {
            if matches!(use_def.prefix, UsePrefix::Core) {
                continue;
            }

            let import_prefix = match use_def.prefix {
                UsePrefix::Pkg => ImportPrefix::Pkg,
                UsePrefix::Core => continue,
                UsePrefix::Self_ => ImportPrefix::Self_,
                UsePrefix::Super(n) => ImportPrefix::Super(n),
            };

            let path_names: Vec<_> = use_def.path.iter().map(|(name, _)| name.clone()).collect();
            if let Ok(resolved) = current_path.resolve_relative(&import_prefix, &path_names) {
                let key = resolved.to_string();
                if !seen.contains(&key) {
                    seen.insert(key);
                    deps.push(resolved);
                }
            }
        }
    }

    deps
}

/// Load a single module from a package.
fn load_module(
    pkg: &Package,
    path: &ModulePath,
    parse: ParseFn,
) -> Result<LoadedModule, BuildError> {
    let source = pkg
        .read_module_source(path)
        .map_err(|e| BuildError::Parse {
            module: path.to_string(),
            error: e.to_string(),
        })?;

    let ast = parse(&source).map_err(|e| BuildError::Parse {
        module: path.to_string(),
        error: e,
    })?;

    Ok(LoadedModule {
        path: path.clone(),
        source,
        ast,
    })
}

/// Compile a loaded module to bytecode with cross-module type checking.
fn compile_loaded_module_with_registry(
    loaded: &LoadedModule,
    file_path: &Path,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    imported_hashes: HashMap<Arc<str>, blake3::Hash>,
) -> Result<CompiledModule, BuildError> {
    let check_result =
        crate::infer::check_module_with_registry(loaded.ast.clone(), module_path, registry);

    if !check_result.is_ok() {
        let errors: Vec<String> = check_result
            .errors
            .iter()
            .map(ToString::to_string)
            .collect();
        return Err(BuildError::TypeCheck {
            module: module_path.to_string(),
            errors,
        });
    }

    let compiled = crate::compiler::compile_module_with_imports_and_source(
        &check_result.module,
        &loaded.source,
        &file_path.display().to_string(),
        imported_hashes,
    )
    .map_err(|e| BuildError::Compile {
        module: module_path.to_string(),
        error: e.to_string(),
    })?;

    Ok(compiled)
}

// Tests are in ambient-cli since they require the parser

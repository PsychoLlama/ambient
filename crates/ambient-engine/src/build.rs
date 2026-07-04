//! Package building and symbol database population.
//!
//! This module provides functionality for compiling Ambient packages
//! and populating the symbol database with the compiled results.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use crate::ast::{ItemKind, Module, UseKind, UsePrefix};
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
    platform_source: &str,
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

    // Core modules compile first: they are ordinary Ambient modules and
    // every user module may reference them.
    let core_compiled = compile_core_modules(&mut registry, &mut module_function_hashes, parse)?;
    all_compiled.merge(&core_compiled);

    // Register the embedder-supplied `platform` declaration module so its
    // abilities are in scope fully-qualified (`platform::Network`) and
    // importable (`use platform::Network;`). Declaration-only: never
    // compiled, and skipped by dependency extraction.
    crate::core_library::register_declaration_module(
        &mut registry,
        &["platform"],
        platform_source,
        parse,
    )
    .map_err(|(module, error)| BuildError::Parse { module, error })?;

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
            let deps = extract_dependencies(&module.ast, path, pkg);
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
///
/// Public so the CLI's diagnostics-oriented build path can share it.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn build_imported_hashes_from_compiled(
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    compiled_hashes: &HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>>,
) -> HashMap<Arc<str>, blake3::Hash> {
    let mut hashes = HashMap::new();

    if let Ok(resolved_imports) = registry.resolve_imports(module_path) {
        // Failed imports were already reported as type errors during
        // checking; only the resolved bindings matter for linking.
        for (local_name, bindings) in resolved_imports.imports {
            // A name can carry both a module and a symbol binding; each
            // populates a different key namespace (`alias.fn` vs `name`), so
            // both apply independently.
            for resolved in bindings {
                match resolved {
                    ResolvedImport::Symbol { from_module, .. } => {
                        if let Some(module_hashes) = compiled_hashes.get(&from_module)
                            && let Some(hash) = module_hashes.get(&local_name)
                        {
                            hashes.insert(local_name.clone(), *hash);
                        }
                    }
                    ResolvedImport::Module(target_path) => {
                        // Whole-module import: every function is callable as
                        // `<alias>.<name>`, which is exactly the key the
                        // compiler builds for a qualified call.
                        if let Some(module_hashes) = compiled_hashes.get(&target_path) {
                            for (fn_name, hash) in module_hashes {
                                hashes.insert(format!("{local_name}.{fn_name}").into(), *hash);
                            }
                        }
                    }
                }
            }
        }
    }

    // Core and platform modules are always in scope under their fully
    // qualified names (`core.List.map`, `platform.Network`), no import
    // required. (Platform is a declaration-only module with no compiled
    // functions, so it never appears in `compiled_hashes` today — but the
    // check keeps the reserved roots symmetric with the type checker's.)
    for (path, module_hashes) in compiled_hashes {
        let root = path.segments().first().map(AsRef::as_ref);
        if root != Some("core") && root != Some("platform") {
            continue;
        }
        for (fn_name, hash) in module_hashes {
            hashes.insert(format!("{path}.{fn_name}").into(), *hash);
        }
    }

    // Trait impl methods dispatch through canonical `uuid::Trait::method`
    // symbols rather than imported names, and the symbols are globally
    // unique (UUID-keyed). Make every already-compiled impl method
    // resolvable so cross-module method calls link.
    for module_hashes in compiled_hashes.values() {
        for (name, hash) in module_hashes {
            if name.contains("::") {
                hashes.insert(Arc::clone(name), *hash);
            }
        }
    }

    hashes
}

/// Collect the enum definitions a module imports (`use pkg::m::{SomeEnum}`).
///
/// Enum constructors compile inline by tag rather than linking by hash,
/// so cross-module enum use hands the compiler the imported definitions
/// themselves — a separate channel from `imported_hashes`.
#[must_use]
pub fn build_imported_enums(
    module_path: &ModulePath,
    registry: &ModuleRegistry,
) -> Vec<crate::ast::EnumDef> {
    let mut enums = Vec::new();
    let Ok(resolved) = registry.resolve_imports(module_path) else {
        return enums;
    };
    for (name, bindings) in resolved.imports {
        for import in bindings {
            let ResolvedImport::Symbol {
                from_module,
                export_kind: crate::module_registry::ExportKind::Enum,
                ..
            } = import
            else {
                continue;
            };
            if let Some(info) = registry.get(&from_module) {
                for item in &info.module.items {
                    if let ItemKind::Enum(def) = &item.kind
                        && def.name == name
                    {
                        enums.push(def.clone());
                    }
                }
            }
        }
    }
    enums
}

/// Collect the constant definitions a module imports (`use pkg::m::{PI}`).
///
/// Constants inline their literal value at each reference site rather than
/// linking by hash, so cross-module constant use hands the compiler the
/// imported definitions themselves — a separate channel from
/// `imported_hashes`, mirroring [`build_imported_enums`].
#[must_use]
pub fn build_imported_constants(
    module_path: &ModulePath,
    registry: &ModuleRegistry,
) -> Vec<crate::ast::ConstDef> {
    let mut constants = Vec::new();
    let Ok(resolved) = registry.resolve_imports(module_path) else {
        return constants;
    };
    for (name, bindings) in resolved.imports {
        for import in bindings {
            let ResolvedImport::Symbol {
                from_module,
                export_kind: crate::module_registry::ExportKind::Const,
                ..
            } = import
            else {
                continue;
            };
            if let Some(info) = registry.get(&from_module) {
                for item in &info.module.items {
                    if let ItemKind::Const(def) = &item.kind
                        && def.name == name
                    {
                        constants.push(def.clone());
                    }
                }
            }
        }
    }
    constants
}

/// Register and compile the embedded core library modules.
///
/// Core modules are registered in the registry under their reserved
/// `core.*` paths (so type checking can see them), compiled through the
/// ordinary pipeline, and their per-module function hashes are recorded
/// into `module_function_hashes` (so calls link).
///
/// Returns the merged compiled core module. The caller merges it into the
/// final build so core functions execute like any others.
///
/// # Errors
///
/// Returns an error if a core module fails to parse, check, or compile —
/// all of which are bugs in the embedded sources rather than user error.
#[allow(clippy::implicit_hasher)]
pub fn compile_core_modules(
    registry: &mut ModuleRegistry,
    module_function_hashes: &mut HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>>,
    parse: impl Fn(&str) -> Result<Module, String>,
) -> Result<CompiledModule, BuildError> {
    let core_paths = crate::core_library::register_core_modules(registry, parse).map_err(
        |(module, error)| BuildError::Parse {
            module: format!("core.{module}"),
            error,
        },
    )?;

    let mut merged = CompiledModule::new();
    for core_path in core_paths {
        let ast = registry
            .get(&core_path)
            .map(|info| info.module.clone())
            .ok_or_else(|| BuildError::PackageOpen(format!("core module {core_path} vanished")))?;

        let check_result =
            crate::infer::check_module_with_registry((*ast).clone(), &core_path, registry);
        if !check_result.is_ok() {
            return Err(BuildError::TypeCheck {
                module: core_path.to_string(),
                errors: check_result
                    .errors
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            });
        }

        let imported_hashes =
            build_imported_hashes_from_compiled(&core_path, registry, module_function_hashes);
        let compiled =
            crate::compiler::compile_module_with_imports(&check_result.module, imported_hashes)
                .map_err(|e| BuildError::Compile {
                    module: core_path.to_string(),
                    error: e.to_string(),
                })?;

        let mut func_hashes = HashMap::new();
        for (name, hash) in &compiled.function_names {
            func_hashes.insert(Arc::clone(name), *hash);
        }

        // Core modules share plain names (`list.map`, `option.map`, ...).
        // The merged artifact binds them fully qualified so they never
        // collide with each other or with user functions.
        let mut compiled = compiled;
        compiled.function_names = compiled
            .function_names
            .iter()
            .map(|(name, hash)| {
                let qualified: Arc<str> = format!("{core_path}.{name}").into();
                (qualified, *hash)
            })
            .collect();

        module_function_hashes.insert(core_path, func_hashes);
        merged.merge(&compiled);
    }

    Ok(merged)
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
    let deps = extract_dependencies(&loaded.ast, path, pkg);
    pkg.add_module(loaded);

    for dep_path in deps {
        load_module_with_deps(pkg, &dep_path, parse)?;
    }

    Ok(())
}

/// The module dependencies a module's `use` statements pull in.
///
/// Each imported name resolves against its parent module, so `use a::b::c`
/// and `use a::b::{c}` both depend on `a::b` (and on submodule `a::b::c` when
/// it exists). Shared with the CLI's package compiler so both loaders agree.
#[must_use]
pub fn extract_dependencies(
    module: &crate::ast::Module,
    current_path: &ModulePath,
    pkg: &Package,
) -> Vec<ModulePath> {
    let mut deps = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |resolved: ModulePath, deps: &mut Vec<ModulePath>| {
        if seen.insert(resolved.to_string()) {
            deps.push(resolved);
        }
    };

    for item in &module.items {
        let ItemKind::Use(use_def) = &item.kind else {
            continue;
        };

        let import_prefix = match use_def.prefix {
            // Core and platform modules are embedded, not loaded from the
            // package tree.
            UsePrefix::Core | UsePrefix::Platform => continue,
            UsePrefix::Pkg => ImportPrefix::Pkg,
            UsePrefix::Self_ => ImportPrefix::Self_,
            UsePrefix::Super(n) => ImportPrefix::Super(n),
        };

        let path_names: Vec<_> = use_def.path.iter().map(|(name, _)| name.clone()).collect();

        // Each imported name resolves against its parent module: `use a::b::c`
        // and `use a::b::{c}` both name `c` under `a::b`. So for each name,
        // the module that must load is either the submodule at the full path
        // (a whole-module import) or the parent module that exports the name
        // (an item import). Load whichever exist; if neither does, load the
        // most specific candidate so the missing module is reported.
        let fulls: Vec<Vec<_>> = match &use_def.kind {
            UseKind::Module => vec![path_names.clone()],
            UseKind::Items(items) => items
                .iter()
                .map(|item_name| {
                    let mut full = path_names.clone();
                    full.push(item_name.clone());
                    full
                })
                .collect(),
        };

        for full in fulls {
            let parent = &full[..full.len().saturating_sub(1)];
            let full_mod = current_path.resolve_relative(&import_prefix, &full).ok();
            let parent_mod = (!parent.is_empty())
                .then(|| current_path.resolve_relative(&import_prefix, parent).ok())
                .flatten();

            let full_exists = full_mod.as_ref().is_some_and(|m| pkg.module_exists(m));
            let parent_exists = parent_mod.as_ref().is_some_and(|m| pkg.module_exists(m));

            if !full_exists && !parent_exists {
                // Neither is a real module; surface the miss on the most
                // specific candidate (the parent for an item import, else the
                // full path).
                if let Some(m) = parent_mod.or(full_mod) {
                    push(m, &mut deps);
                }
            } else {
                if let Some(m) = full_mod.filter(|_| full_exists) {
                    push(m, &mut deps);
                }
                if let Some(m) = parent_mod.filter(|_| parent_exists) {
                    push(m, &mut deps);
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

    let source_file = file_path.display().to_string();
    let compiled = crate::compiler::compile_module_with_options(
        &check_result.module,
        crate::compiler::CompileOptions {
            source: Some(&loaded.source),
            source_file: Some(&source_file),
            imported_hashes: Some(imported_hashes),
            imported_enums: build_imported_enums(module_path, registry),
            imported_constants: build_imported_constants(module_path, registry),
            prelude_abilities: &[],
        },
    )
    .map_err(|e| BuildError::Compile {
        module: module_path.to_string(),
        error: e.to_string(),
    })?;

    Ok(compiled)
}

// Tests are in ambient-cli since they require the parser

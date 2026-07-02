//! Run command implementation.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};

use ambient_engine::abilities::{
    register_console, register_execute, register_log, register_network, ConsoleConfig,
    ExecuteConfig, LogConfig, NetworkConfig,
};
use ambient_engine::ast::{ItemKind, UsePrefix};
use ambient_engine::build::{build_imported_hashes_from_compiled, compile_core_modules};
use ambient_engine::compiler::CompiledModule;
use ambient_engine::format::format_value_colored;
use ambient_engine::module_path::{ImportPrefix, ModulePath};
use ambient_engine::module_registry::ModuleRegistry;
use ambient_engine::package::{LoadedModule, Package};
use ambient_engine::runtime_config::RuntimeConfig;
use ambient_engine::store::Store;
use ambient_engine::vm::Vm;

use crate::diagnostic::print_diagnostic;

/// Run an Ambient package or pre-compiled artifact.
///
/// If `path` is a directory (or contains an `ambient.toml`), runs the package.
/// If `path` is a `.ambient` file, runs the pre-compiled artifact pack.
pub fn cmd_run(path: &Path, entry: &str) -> Result<()> {
    let compiled = load_compiled(path)?;
    run_compiled(&compiled, entry)
}

/// Load a compiled module from a path.
///
/// Handles both packages (directories with `ambient.toml`) and
/// pre-compiled `.ambient` artifact packs.
fn load_compiled(path: &Path) -> Result<CompiledModule> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    if ext == "ambient" {
        // Load a pre-compiled artifact pack. Function hashes are recomputed
        // from the object bytes, so a tampered artifact fails to load.
        let bytes = fs::read(path).context("failed to read file")?;
        let pack = ambient_engine::store::Pack::decode(&bytes)
            .map_err(|e| anyhow::anyhow!("invalid artifact {}: {e}", path.display()))?;
        CompiledModule::from_pack(&pack)
            .map_err(|e| anyhow::anyhow!("invalid artifact {}: {e}", path.display()))
    } else if path.is_dir() || path.join("ambient.toml").exists() {
        // Load package.
        compile_package(path)
    } else {
        bail!(
            "expected a directory with ambient.toml or a .ambient file, got: {}",
            path.display()
        );
    }
}

/// Compile a package from its root directory.
#[allow(clippy::arc_with_non_send_sync)]
fn compile_package(path: &Path) -> Result<CompiledModule> {
    // Open package (validates manifest and entry point).
    let mut pkg = Package::open(path)
        .with_context(|| format!("failed to open package at {}", path.display()))?;

    // Load the main module and all its dependencies.
    let main_path = ModulePath::root();
    load_module_with_deps(&mut pkg, &main_path)?;

    // Build module registry with all loaded modules.
    let mut registry = ModuleRegistry::new();
    for module in pkg.all_modules() {
        registry.register(&module.path, Arc::new(module.ast.clone()));
    }

    // Get modules in topological order (dependencies first).
    let module_order = get_compilation_order(&pkg, &main_path);

    // Compile modules in dependency order, tracking function hashes.
    let mut all_compiled = CompiledModule::new();
    let mut module_function_hashes: HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>> =
        HashMap::new();

    // Core modules compile first: they are ordinary Ambient modules and
    // every user module may reference them.
    let core_compiled = compile_core_modules(&mut registry, &mut module_function_hashes, |s| {
        ambient_parser::parse(s).map_err(|e| e.to_string())
    })
    .map_err(|e| anyhow::anyhow!("core library failed to build: {e}"))?;
    all_compiled.merge(&core_compiled);

    for module_path in module_order {
        let module = pkg
            .get_module(&module_path)
            .ok_or_else(|| anyhow::anyhow!("module not found: {}", module_path))?;
        let file_path = pkg.module_file_path(&module_path);

        // Build imported function hashes from already-compiled dependencies.
        let imported_hashes =
            build_imported_hashes_from_compiled(&module_path, &registry, &module_function_hashes);

        let compiled = compile_loaded_module_with_registry(
            module,
            &file_path,
            &module_path,
            &registry,
            imported_hashes,
        )?;

        // Record this module's function hashes for dependents.
        let mut func_hashes = HashMap::new();
        for (name, hash) in &compiled.function_names {
            func_hashes.insert(Arc::clone(name), *hash);
        }
        module_function_hashes.insert(module_path, func_hashes);

        // Merge into the final module.
        all_compiled.merge(&compiled);
    }

    // Persist the build to the package-local content-addressed store.
    // Failure to persist is a warning, not a failed run.
    match ambient_engine::disk_store::DiskStore::open(path.join(".ambient").join("store")) {
        Ok(disk) => {
            if let Err(e) = disk.put_module(&all_compiled) {
                eprintln!("warning: failed to persist build to store: {e}");
            }
        }
        Err(e) => eprintln!("warning: failed to open package store: {e}"),
    }

    Ok(all_compiled)
}

/// Get modules in topological order (dependencies first).
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

/// Load a module and all its dependencies recursively.
fn load_module_with_deps(pkg: &mut Package, path: &ModulePath) -> Result<()> {
    // Skip if already loaded.
    if pkg.is_loaded(path) {
        return Ok(());
    }

    // Load the module.
    let loaded = load_module(pkg, path)?;

    // Extract dependencies from use statements.
    let deps = extract_dependencies(&loaded.ast, path);

    // Add module to package.
    pkg.add_module(loaded);

    // Recursively load dependencies.
    for dep_path in deps {
        load_module_with_deps(pkg, &dep_path)?;
    }

    Ok(())
}

/// Extract module dependencies from use statements.
fn extract_dependencies(
    module: &ambient_engine::ast::Module,
    current_path: &ModulePath,
) -> Vec<ModulePath> {
    let mut deps = Vec::new();
    let mut seen = HashSet::new();

    for item in &module.items {
        if let ItemKind::Use(use_def) = &item.kind {
            // Skip core library imports - they're handled separately.
            if matches!(use_def.prefix, UsePrefix::Core) {
                continue;
            }

            // Resolve the import path.
            let import_prefix = match use_def.prefix {
                UsePrefix::Pkg => ImportPrefix::Pkg,
                UsePrefix::Core => continue, // Already handled above
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
fn load_module(pkg: &Package, path: &ModulePath) -> Result<LoadedModule> {
    let source = pkg.read_module_source(path)?;
    let file_path = pkg.module_file_path(path);

    // Parse the module.
    let ast = match ambient_parser::parse(&source) {
        Ok(m) => m,
        Err(e) => {
            print_diagnostic(&source, &file_path, &e);
            bail!("parse error in {}", file_path.display());
        }
    };

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
) -> Result<CompiledModule> {
    // Type check with cross-module support.
    let check_result = ambient_engine::infer::check_module_with_registry(
        loaded.ast.clone(),
        module_path,
        registry,
    );

    if !check_result.is_ok() {
        for error in &check_result.errors {
            print_diagnostic(&loaded.source, file_path, error);
        }
        bail!(
            "Found {} type error(s) in {}",
            check_result.errors.len(),
            file_path.display()
        );
    }

    // Compile with debug info and imported hashes.
    let compiled = ambient_engine::compiler::compile_module_with_imports_and_source(
        &check_result.module,
        &loaded.source,
        &file_path.display().to_string(),
        imported_hashes,
    )
    .map_err(|e| anyhow::anyhow!("compile error at {}: {e}", file_path.display()))?;

    Ok(compiled)
}

/// Run a compiled module.
fn run_compiled(compiled: &CompiledModule, entry: &str) -> Result<()> {
    // Create tokio runtime for async operations (Remote ability).
    let runtime = tokio::runtime::Runtime::new().context("failed to create async runtime")?;

    // Create VM with native abilities (Console, Time, Random, Log).
    let config = RuntimeConfig::native();
    let mut vm = Vm::with_runtime(&config);

    // Create store for function dependencies (used by the Execute ability).
    // add_module registers canonical objects so functions can be shipped.
    let mut store = Store::new();
    store.add_module(compiled);
    let store = Arc::new(std::sync::Mutex::new(store));

    // Register Network ability for TCP operations.
    register_network(
        &mut vm,
        NetworkConfig {
            runtime: runtime.handle().clone(),
        },
    );

    // Register Execute ability for server-side function execution.
    // Grant output abilities (Console, Log) to executed code: remotely
    // received functions may print/log on this host, but get no Network,
    // Time, Random, or (recursive) Execute access.
    register_execute(
        &mut vm,
        ExecuteConfig {
            store: Arc::clone(&store),
            grants: Some(Arc::new(|exec_vm: &mut Vm| {
                register_console(exec_vm, ConsoleConfig::default());
                register_log(exec_vm, LogConfig::default());
            })),
        },
    );

    // Load all functions into the VM.
    for func in compiled.functions.values() {
        vm.load_function(func.clone());
    }

    // Find entry point.
    let entry_hash = compiled
        .function_names
        .get(entry)
        .ok_or_else(|| anyhow::anyhow!("entry function `{entry}` not found"))?;

    // Execute with stack trace support.
    let result = vm.call_with_trace(entry_hash, Vec::new());

    match result {
        Ok(value) => {
            // Print result if not unit.
            if !matches!(value, ambient_engine::value::Value::Unit) {
                println!("{}", format_value_colored(&value));
            }
            Ok(())
        }
        Err(runtime_error) => {
            // Print rich error with stack trace.
            eprintln!("{runtime_error}");
            bail!("runtime error");
        }
    }
}

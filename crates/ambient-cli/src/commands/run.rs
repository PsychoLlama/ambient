//! Run command implementation.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use ambient_engine::build::{
    build_imported_hashes_from_compiled, compile_core_modules, extract_dependencies,
};
use ambient_engine::compiler::CompiledModule;
use ambient_engine::format::format_value_colored;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;
use ambient_engine::package::{LoadedModule, Package};
use ambient_platform::process::ProcessEvent;

use super::host::RuntimeHost;
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
/// Handles packages (directories with `ambient.toml`), pre-compiled
/// `.ambient` artifact packs, and bare `.ab` source files.
pub(super) fn load_compiled(path: &Path) -> Result<CompiledModule> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    if ext == "ab" && path.is_file() {
        // Compile a bare source file against the core library.
        let source = super::read_source(path)?;
        return super::compile_source(&source, path);
    }

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
pub(super) fn compile_package(path: &Path) -> Result<CompiledModule> {
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

/// Load a module and all its dependencies recursively.
fn load_module_with_deps(pkg: &mut Package, path: &ModulePath) -> Result<()> {
    // Skip if already loaded.
    if pkg.is_loaded(path) {
        return Ok(());
    }

    // Load the module.
    let loaded = load_module(pkg, path)?;

    // Extract dependencies from use statements.
    let deps = extract_dependencies(&loaded.ast, path, pkg);

    // Add module to package.
    pkg.add_module(loaded);

    // Recursively load dependencies.
    for dep_path in deps {
        load_module_with_deps(pkg, &dep_path)?;
    }

    Ok(())
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
    // Type check with cross-module support and the platform prelude.
    let prelude = super::platform_prelude()?;
    let check_result = ambient_engine::infer::check_module_with_registry_and_resolver(
        loaded.ast.clone(),
        module_path,
        registry,
        super::prelude_resolver(&prelude),
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
    let source_file = file_path.display().to_string();
    let compiled = ambient_engine::compiler::compile_module_with_options(
        &check_result.module,
        ambient_engine::compiler::CompileOptions {
            source: Some(&loaded.source),
            source_file: Some(&source_file),
            imported_hashes: Some(imported_hashes),
            imported_enums: ambient_engine::build::build_imported_enums(module_path, registry),
            prelude_abilities: &prelude,
        },
    )
    .map_err(|e| anyhow::anyhow!("compile error at {}: {e}", file_path.display()))?;

    Ok(compiled)
}

/// Run a compiled module.
///
/// The entry runs as the initial deploy pass of a process runtime. A
/// program that spawns no processes behaves exactly as before: the
/// entry runs to completion and the command exits. A program that
/// spawns processes keeps running until every process has exited.
fn run_compiled(compiled: &CompiledModule, entry: &str) -> Result<()> {
    // `run` is quiet about routine lifecycle; only failures print.
    let events = Arc::new(|event: &ProcessEvent| match event {
        ProcessEvent::Crashed {
            name,
            error,
            restarting,
        } => {
            eprintln!("process `{name}` crashed: {error}");
            if *restarting {
                eprintln!("process `{name}` restarting with fresh state");
            } else {
                eprintln!("process `{name}` exceeded its fault budget; giving up");
            }
        }
        ProcessEvent::InitFailed { name, error } => {
            eprintln!("process `{name}` failed to initialize: {error}");
        }
        _ => {}
    });

    let host = RuntimeHost::new(events)?;

    match host.deploy(compiled, entry) {
        Ok(outcome) => {
            // Print result if not unit.
            if !matches!(outcome.value, ambient_engine::value::Value::Unit) {
                println!("{}", format_value_colored(&outcome.value));
            }
        }
        Err(runtime_error) => {
            // Print rich error with stack trace.
            eprintln!("{runtime_error}");
            bail!("runtime error");
        }
    }

    // Block until the process tree (if any) winds down.
    host.runtime().wait_all();
    Ok(())
}

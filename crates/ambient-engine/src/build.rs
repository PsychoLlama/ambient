//! Package building and symbol database population.
//!
//! This is the *single* package pipeline: load every module, register the
//! core/platform/user modules in one registry, canonicalize references
//! (the resolve pass), order modules by their resolved dependencies, and
//! check + compile each one. `ambient run`, `ambient compile`, and
//! `ambient dev` are all frontends over [`build_package`]; behavior that
//! must differ between them is expressed in [`BuildOptions`], never by
//! forking the pipeline.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::ast::{ItemKind, Module};
use crate::compiler::CompiledModule;
use crate::infer::BoxedTypeError;
use crate::module_path::ModulePath;
use crate::module_registry::{ExportKind, ModuleRegistry};
use crate::package::{LoadedModule, Package};
use crate::symbol_db::SymbolDb;

/// Progress callback for reporting build progress.
///
/// Called with (module name, current, total) for each module.
pub type ProgressCallback<'a> = &'a dyn Fn(&str, usize, usize);

/// A parse failure the build can render with source context: message, byte
/// span, and optional note.
///
/// Engine-local so `ambient-engine` needn't depend on the parser (the
/// dependency runs the other way). The caller's parse function fills this
/// from `ambient_parser::ParseError`, and the CLI converts it back to a
/// rendered diagnostic — the same spanned rendering `ambient check` gives.
#[derive(Debug, Clone)]
pub struct ParseFailure {
    /// The primary message.
    pub message: String,
    /// Byte offset range in the module source.
    pub span: (u32, u32),
    /// Optional context/note.
    pub context: Option<String>,
}

/// Parse function type for parsing source code into an AST.
pub type ParseFn = fn(&str) -> Result<Module, ParseFailure>;

/// Knobs for a package build.
#[derive(Default)]
pub struct BuildOptions<'a> {
    /// Source of the embedder's `platform` declaration module (ability
    /// bindings interface). Empty disables platform registration.
    pub platform_source: &'a str,
    /// Embedder-resolved prelude abilities for the compiler (host binding
    /// identities). The type checker resolves abilities through the
    /// registry; this is the compiler's separate concern.
    pub prelude_abilities: &'a [Arc<crate::ability_resolver::DynAbility>],
    /// Optional callback for reporting per-module progress.
    pub progress: Option<ProgressCallback<'a>>,
}

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
///
/// The `Parse` and `TypeCheck` variants carry the offending module's source
/// and file path alongside structured (spanned) errors, so a frontend can
/// render them with source context — byte-identically to `ambient check`.
/// Message-only failures (opening the package, codegen, embedded
/// core/platform modules) have no user source to point at and carry just a
/// message.
#[derive(Debug)]
pub enum BuildError {
    /// Failed to open the package.
    PackageOpen(String),
    /// A user module failed to parse. The failure is boxed to keep the
    /// `Result`'s error variant small.
    Parse {
        module: String,
        path: PathBuf,
        source: String,
        error: Box<ParseFailure>,
    },
    /// A user module failed to type-check.
    TypeCheck {
        module: String,
        path: PathBuf,
        source: String,
        errors: Vec<BoxedTypeError>,
    },
    /// Codegen failed, or an embedded core/platform module failed to build.
    /// Compiler-internal: no user source to render against.
    Compile { module: String, error: String },
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PackageOpen(msg) => write!(f, "failed to open package: {msg}"),
            Self::Parse { module, error, .. } => {
                write!(f, "parse error in {module}: {}", error.message)
            }
            Self::TypeCheck { module, errors, .. } => {
                let joined = errors
                    .iter()
                    .map(|e| e.kind.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "type errors in {module}: {joined}")
            }
            Self::Compile { module, error } => write!(f, "compile error in {module}: {error}"),
        }
    }
}

impl std::error::Error for BuildError {}

/// Build an Ambient package and populate the symbol database.
///
/// Pipeline:
/// 1. Load and parse every `.ab` file under `src/`.
/// 2. Register core modules (compiling them), the `platform` declaration
///    module, and every package module in one [`ModuleRegistry`].
/// 3. Run the resolve pass over each package module: canonicalize every
///    cross-module reference and collect the true dependency graph.
/// 4. Compile modules in dependency order, linking canonical names to
///    content-addressed hashes.
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
    options: &BuildOptions<'_>,
) -> Result<BuildResult, BuildError> {
    // Open package (validates manifest and entry point).
    let mut pkg = Package::open(path).map_err(|e| BuildError::PackageOpen(e.to_string()))?;

    let package_name = pkg.manifest().name.clone();

    // Load every module in the package. Loading everything (rather than
    // chasing `use` statements from `main`) is what makes directory
    // namespaces and inline `pkg::a::b::f()` references work: the module
    // graph is defined by the filesystem, and the *dependency* graph by
    // the resolve pass below.
    load_all_modules(&mut pkg, parse)?;

    let mut registry = ModuleRegistry::new();

    // Core modules compile first: they are ordinary Ambient modules and
    // every user module may reference them. Core registration only needs a
    // string on failure (a parse error there is a compiler bug, not user
    // error), so adapt the richer `ParseFn`.
    let parse_str = |s: &str| parse(s).map_err(|e| e.message);
    let mut all_compiled = CompiledModule::new();
    let mut module_function_hashes: HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>> =
        HashMap::new();
    let core_compiled =
        compile_core_modules(&mut registry, &mut module_function_hashes, parse_str)?;
    all_compiled.merge(&core_compiled);

    // Register the embedder-supplied `platform` declaration module so its
    // abilities are in scope fully-qualified (`platform::Network`) and
    // importable (`use platform::Network;`). Declaration-only: never
    // compiled.
    if !options.platform_source.is_empty() {
        crate::core_library::register_declaration_module(
            &mut registry,
            &["platform"],
            options.platform_source,
            parse_str,
        )
        .map_err(|(module, error)| BuildError::Compile { module, error })?;
    }

    // Register every package module, then canonicalize. Resolution needs
    // all modules registered (imports may point anywhere in the package);
    // the resolved ASTs then *replace* the raw ones in the registry so
    // cross-module signature hydration sees canonical references too.
    for module in pkg.all_modules() {
        registry.register(&module.path, Arc::new(module.ast.clone()));
    }
    let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut paths_by_key: BTreeMap<String, ModulePath> = BTreeMap::new();
    for module in pkg.all_modules_mut() {
        // Block-use failures surface as diagnostics when the module
        // checks (the pass runs again there); only the dependency set
        // matters here.
        let outcome = crate::resolve::resolve_module(&mut module.ast, &module.path, &registry);
        deps.insert(
            module.path.to_string(),
            outcome.deps.iter().map(ToString::to_string).collect(),
        );
        paths_by_key.insert(module.path.to_string(), module.path.clone());
    }
    for module in pkg.all_modules() {
        registry.register(&module.path, Arc::new(module.ast.clone()));
    }

    // Compile in dependency order (dependencies first).
    let module_order = compilation_order(&deps);
    let total_modules = module_order.len();

    // Open symbol database in build directory (create if needed).
    let build_dir = path.join("build");
    fs::create_dir_all(&build_dir).ok();
    let db_path = build_dir.join("symbols.db");
    let mut symbol_db = SymbolDb::open(&db_path).ok();

    for (idx, module_key) in module_order.iter().enumerate() {
        let module_path = paths_by_key
            .get(module_key)
            .cloned()
            .ok_or_else(|| BuildError::PackageOpen(format!("module not found: {module_key}")))?;
        let module = pkg
            .get_module(&module_path)
            .ok_or_else(|| BuildError::PackageOpen(format!("module not found: {module_path}")))?;
        let file_path = pkg.module_file_path(&module_path);

        // Report progress
        if let Some(ref cb) = options.progress {
            cb(module_key, idx + 1, total_modules);
        }

        let compiled = compile_loaded_module_with_registry(
            module,
            &file_path,
            &module_path,
            &registry,
            linking_table(&module_function_hashes),
            options.prelude_abilities,
        )?;

        // Populate symbol database with compiled module.
        if let Some(ref mut db) = symbol_db {
            let visibility = extract_function_visibility(&module.ast);

            // Clean up old symbols for this module before repopulating.
            let _ = db.remove_module(module_key);

            // Populate with new symbols.
            let _ = db.populate_from_module(&compiled, &package_name, module_key, &visibility);
        }

        // Record this module's function hashes for dependents, keyed by
        // their bare names (the linking table qualifies them itself).
        let mut func_hashes = HashMap::new();
        for (name, hash) in &compiled.function_names {
            func_hashes.insert(Arc::clone(name), *hash);
        }
        module_function_hashes.insert(module_path.clone(), func_hashes);

        // Merge into the final module, qualifying this module's function
        // names with its module path (`math::gcd`) — the canonical identity
        // (`resolution_key`), matching how core modules are merged below.
        // Package modules were previously merged bare, which surfaced as
        // unqualified store names (`gcd`) and silently clobbered same-named
        // functions across modules in the merged map. Impl-method dispatch
        // symbols are already globally unique (`<uuid>::Trait::method`), so
        // they pass through unqualified like in `linking_table`.
        let mut compiled = compiled;
        compiled.function_names = compiled
            .function_names
            .iter()
            .map(|(name, hash)| {
                let qualified: Arc<str> = if name.contains("::") {
                    Arc::clone(name)
                } else {
                    format!("{module_path}::{name}").into()
                };
                (qualified, *hash)
            })
            .collect();
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

/// Load and parse every `.ab` file under the package's `src/` directory.
fn load_all_modules(pkg: &mut Package, parse: ParseFn) -> Result<(), BuildError> {
    let mut paths = discover_module_paths(&pkg.src_path())
        .map_err(|e| BuildError::PackageOpen(format!("failed to scan src: {e}")))?;
    paths.sort_by_key(ToString::to_string);
    for module_path in paths {
        if module_path.collides_with_reserved_root() {
            return Err(BuildError::PackageOpen(format!(
                "module `{module_path}` collides with the reserved `{}` namespace; rename the file",
                module_path.segments()[0]
            )));
        }
        if pkg.is_loaded(&module_path) {
            continue;
        }
        let loaded = load_module(pkg, &module_path, parse)?;
        pkg.add_module(loaded);
    }
    Ok(())
}

/// Every module path under a source directory: each `.ab` file, mapped
/// through the canonical file↔module mapping.
///
/// # Errors
///
/// Returns an error if the directory tree cannot be read.
pub fn discover_module_paths(src: &Path) -> std::io::Result<Vec<ModulePath>> {
    let mut found = Vec::new();
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("ab")
                && let Ok(relative) = path.strip_prefix(src)
                && let Some(module_path) = ModulePath::from_relative_file_path(relative)
            {
                found.push(module_path);
            }
        }
    }
    Ok(found)
}

/// Topologically order modules by their resolved dependencies
/// (dependencies first). Modules outside the package (core, platform) are
/// skipped; cycles fall back to name order and surface as link errors at
/// the offending call sites.
#[allow(clippy::items_after_statements)]
fn compilation_order(deps: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    let mut order = Vec::new();
    let mut visited = HashSet::new();

    fn visit(
        key: &str,
        deps: &BTreeMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        order: &mut Vec<String>,
    ) {
        if visited.contains(key) {
            return;
        }
        visited.insert(key.to_string());
        if let Some(module_deps) = deps.get(key) {
            for dep in module_deps {
                // Only package modules participate; core/platform compile
                // ahead of the package.
                if deps.contains_key(dep) {
                    visit(dep, deps, visited, order);
                }
            }
            order.push(key.to_string());
        }
    }

    for key in deps.keys() {
        visit(key, deps, &mut visited, &mut order);
    }
    order
}

/// The linking table for a module about to compile: every already-compiled
/// function, bound under its canonical name.
///
/// - Ordinary functions bind as `<module path>::<name>` (`core::List::map`,
///   `utils::helper`). The resolve pass rewrites every cross-module
///   reference to exactly this key.
/// - Impl-method dispatch symbols (`<uuid>::Trait::method`) are globally
///   unique and bind as-is, so cross-module method calls link.
///
/// Module-local calls use the module's own bare names, which the compiler
/// seeds itself — they never pass through this table.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn linking_table(
    compiled_hashes: &HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>>,
) -> HashMap<Arc<str>, blake3::Hash> {
    let mut table = HashMap::new();
    for (path, module_hashes) in compiled_hashes {
        for (name, hash) in module_hashes {
            if name.contains("::") {
                table.insert(Arc::clone(name), *hash);
            } else {
                table.insert(format!("{path}::{name}").into(), *hash);
            }
        }
    }
    table
}

/// Collect the enum definitions a module imports (`use pkg::m::{SomeEnum}`).
///
/// Enum constructors compile inline by tag rather than linking by hash,
/// so cross-module enum use hands the compiler the imported definitions
/// themselves — a separate channel from the linking table.
#[must_use]
pub fn build_imported_enums(
    module_path: &ModulePath,
    registry: &ModuleRegistry,
) -> Vec<crate::ast::EnumDef> {
    let mut enums = Vec::new();
    let scope = registry.build_module_scope(module_path);
    for imports in scope.items.values() {
        for import in imports {
            if import.kind != ExportKind::Enum {
                continue;
            }
            if let Some(info) = registry.get(&import.module) {
                for item in &info.module.items {
                    if let ItemKind::Enum(def) = &item.kind
                        && def.name == import.name
                    {
                        enums.push(def.clone());
                    }
                }
            }
        }
    }
    enums
}

/// Collect every foreign constant in the build, keyed canonically
/// (`utils::MAX`). Constants inline their literal value at each reference
/// site rather than linking by hash, so the compiler needs the
/// definitions themselves — a separate channel from the linking table,
/// mirroring [`build_imported_enums`]. All public constants are provided
/// (not just imported ones) because inline `pkg::utils::MAX` references
/// need no import.
#[must_use]
pub fn build_foreign_constants(
    module_path: &ModulePath,
    registry: &ModuleRegistry,
) -> Vec<(Arc<str>, crate::ast::ConstDef)> {
    let mut constants = Vec::new();
    for info in registry.all_modules() {
        if &info.path == module_path {
            continue;
        }
        for item in &info.module.items {
            if let ItemKind::Const(def) = &item.kind
                && def.is_public
            {
                let key: Arc<str> = format!("{}::{}", info.path, def.name).into();
                constants.push((key, def.clone()));
            }
        }
    }
    constants
}

/// Register and compile the embedded core library modules.
///
/// Core modules are registered in the registry under their reserved
/// `core::*` paths (so type checking can see them), compiled through the
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
        |(module, error)| BuildError::Compile {
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
            // A core module failing to type-check is a compiler bug, not user
            // error, so there is no user source to render against.
            let joined = check_result
                .errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(BuildError::Compile {
                module: core_path.to_string(),
                error: joined,
            });
        }

        let compiled = crate::compiler::compile_module_with_imports(
            &check_result.module,
            linking_table(module_function_hashes),
        )
        .map_err(|e| BuildError::Compile {
            module: core_path.to_string(),
            error: e.to_string(),
        })?;

        let mut func_hashes = HashMap::new();
        for (name, hash) in &compiled.function_names {
            func_hashes.insert(Arc::clone(name), *hash);
        }

        // Core modules share plain names (`list::map`, `option::map`, ...).
        // The merged artifact binds them fully qualified so they never
        // collide with each other or with user functions.
        let mut compiled = compiled;
        compiled.function_names = compiled
            .function_names
            .iter()
            .map(|(name, hash)| {
                let qualified: Arc<str> = format!("{core_path}::{name}").into();
                (qualified, *hash)
            })
            .collect();

        module_function_hashes.insert(core_path, func_hashes);
        merged.merge(&compiled);
    }

    Ok(merged)
}

/// Load a single module from a package.
fn load_module(
    pkg: &Package,
    path: &ModulePath,
    parse: ParseFn,
) -> Result<LoadedModule, BuildError> {
    let source = pkg
        .read_module_source(path)
        .map_err(|e| BuildError::Compile {
            module: path.to_string(),
            error: format!("failed to read source: {e}"),
        })?;

    let file_path = pkg.module_file_path(path);
    let ast = parse(&source).map_err(|error| BuildError::Parse {
        module: path.to_string(),
        path: file_path,
        source: source.clone(),
        error: Box::new(error),
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
    prelude_abilities: &[Arc<crate::ability_resolver::DynAbility>],
) -> Result<CompiledModule, BuildError> {
    let check_result =
        crate::infer::check_module_with_registry(loaded.ast.clone(), module_path, registry);

    if !check_result.is_ok() {
        return Err(BuildError::TypeCheck {
            module: module_path.to_string(),
            path: file_path.to_path_buf(),
            source: loaded.source.clone(),
            errors: check_result.errors,
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
            imported_constants: build_foreign_constants(module_path, registry),
            prelude_abilities,
            foreign_abilities: crate::infer::resolve_registry_abilities(registry),
        },
    )
    .map_err(|e| BuildError::Compile {
        module: module_path.to_string(),
        error: e.to_string(),
    })?;

    Ok(compiled)
}

// Tests are in ambient-cli since they require the parser

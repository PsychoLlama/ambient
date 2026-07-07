//! Package building.
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
use crate::fqn::{Fqn, NameKey};
use crate::infer::BoxedTypeError;
use crate::module_path::ModulePath;
use crate::module_registry::{ExportKind, ModuleRegistry};
use crate::package::{LoadedModule, Package};

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
    /// Source of the embedder's `core::system` declaration module (ability
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
    /// The canonical [`NameKey`] linking table for the whole build (core +
    /// package). Consumers that compile *additional* modules against this
    /// build — the REPL, notably — pass it as `imported_hashes`.
    pub link_table: HashMap<NameKey, blake3::Hash>,
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

/// Build an Ambient package.
///
/// Pipeline:
/// 1. Load and parse every `.ab` file under `src/`.
/// 2. Register core modules (compiling them), the `core::system`
///    declaration module, and every package module in one [`ModuleRegistry`].
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
    // Scope every user item's `Fqn` under the package name (`workspace::<name>`);
    // core modules are `Builtin`-scoped regardless. Set before any
    // resolve/check/link so all three mint identical identities.
    registry.set_workspace_name(package_name.clone());

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

    // Register the embedder-supplied `core::system` declaration module so
    // its abilities are in scope fully-qualified (`core::system::Network`)
    // and importable (`use core::system::Network;`). Declaration-only:
    // never compiled.
    if !options.platform_source.is_empty() {
        crate::core_library::register_declaration_module(
            &mut registry,
            &["core", "system"],
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
        // Reconcile the `Fqn`-based dependency edges with this
        // `ModulePath`-keyed ordering graph: a `ModuleId` renders as
        // `workspace::<pkg>::a::top`, but the graph keys on `a::top`.
        deps.insert(
            module.path.to_string(),
            outcome
                .deps
                .iter()
                .map(crate::fqn::ModuleId::module_path_string)
                .collect(),
        );
        paths_by_key.insert(module.path.to_string(), module.path.clone());
    }
    for module in pkg.all_modules() {
        registry.register(&module.path, Arc::new(module.ast.clone()));
    }

    // Compile in dependency order (dependencies first).
    let module_order = compilation_order(&deps);
    let total_modules = module_order.len();

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
            linking_table(&module_function_hashes, &registry),
            options.prelude_abilities,
        )?;

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
        compiled.function_names = qualify_names(&compiled.function_names, &module_path, &registry);
        compiled.const_names = qualify_names(&compiled.const_names, &module_path, &registry);
        all_compiled.merge(&compiled);
    }

    Ok(BuildResult {
        compiled: all_compiled,
        module_count: total_modules,
        package_name,
        link_table: linking_table(&module_function_hashes, &registry),
    })
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
/// - Ordinary functions bind under their [`Fqn`] ([`NameKey::Item`]);
///   `core::collections::List::map`, `utils::helper`. The resolve pass
///   rewrites every cross-module reference to exactly this key.
/// - Impl-method dispatch symbols (`<uuid>::Trait::method`) are globally
///   unique content symbols and bind as-is under [`NameKey::Bare`], so
///   cross-module method calls link.
///
/// Module-local calls use the module's own bare names, which the compiler
/// seeds itself — they never pass through this table.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn linking_table(
    compiled_hashes: &HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>>,
    registry: &ModuleRegistry,
) -> HashMap<NameKey, blake3::Hash> {
    let mut table = HashMap::new();
    for (path, module_hashes) in compiled_hashes {
        for (name, hash) in module_hashes {
            // A content-addressed dispatch symbol keeps its bare identity;
            // an ordinary function keys under its `Fqn`.
            let key = if name.contains("::") {
                NameKey::Bare(Arc::clone(name))
            } else {
                NameKey::Item(registry.fqn(path, &[Arc::clone(name)]))
            };
            table.insert(key, *hash);
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

/// Collect every foreign constant's value-object hash, keyed canonically by
/// `Fqn` (`utils::MAX`). A `const` links by hash exactly like a function: the
/// consumer emits a `LoadObject` of this hash, and the defining module ships
/// the value object itself (content-addressed, deduplicated). This is the
/// const analogue of the [`linking_table`] — a name→hash channel, not an
/// AST-replication one. All public constants are provided (not just imported
/// ones) because inline `pkg::utils::MAX` references need no import.
///
/// A value object's hash derives only from the const's value, never its name,
/// so this recomputes exactly the hash the defining module's compile produced.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn build_foreign_const_hashes(
    module_path: &ModulePath,
    registry: &ModuleRegistry,
) -> HashMap<NameKey, blake3::Hash> {
    let mut hashes = HashMap::new();
    for info in registry.all_modules() {
        if &info.path == module_path {
            continue;
        }
        for item in &info.module.items {
            if let ItemKind::Const(def) = &item.kind
                && def.is_public
                && let Some(value) = crate::const_eval::literal_value(&def.value)
                && let Ok(object) = crate::object::value_object(&value)
            {
                let key = NameKey::Item(registry.fqn(&info.path, &[Arc::clone(&def.name)]));
                hashes.insert(key, object.hash());
            }
        }
    }
    hashes
}

/// Collect every foreign unit struct in the build, as canonical
/// `<module>::Origin` keys. A unit struct is a value (constructed by its bare
/// name) as well as a type; the compiler inlines each reference to an empty
/// record value rather than linking by hash, so it only needs the key — a
/// separate channel from the linking table, mirroring
/// [`build_foreign_const_hashes`]. All public unit structs are provided (not
/// just imported ones) because inline `pkg::shapes::Origin` references need
/// no import.
#[must_use]
pub fn build_foreign_unit_structs(
    module_path: &ModulePath,
    registry: &ModuleRegistry,
) -> Vec<NameKey> {
    let mut keys = Vec::new();
    for info in registry.all_modules() {
        if &info.path == module_path {
            continue;
        }
        for item in &info.module.items {
            if let ItemKind::Struct(def) = &item.kind
                && def.is_public
                && def.is_unit()
            {
                keys.push(NameKey::Item(
                    registry.fqn(&info.path, &[Arc::clone(&def.name)]),
                ));
            }
        }
    }
    keys
}

/// Collect every foreign enum's variant constructors in the build, keyed by
/// their canonical two-segment `Fqn(enum_module, [Enum, Variant])`.
///
/// Variant construction inlines a `MakeEnum` tag rather than linking by
/// hash, so — like [`build_imported_enums`] — the compiler needs the
/// tag/payload layout, not a name→hash entry. This is the *qualified*
/// channel, distinct from [`build_imported_enums`]'s bare one: a
/// fully-qualified (`core::Option::Some`) or explicit-enum
/// (`pkg::shapes::Shape::Circle`) reference resolves to an `Fqn` looked up
/// here. All public enums are provided (not just imported ones) because
/// inline qualified references need no import.
#[must_use]
pub fn build_foreign_enum_variants(
    module_path: &ModulePath,
    registry: &ModuleRegistry,
) -> Vec<(Fqn, crate::compiler::VariantInfo)> {
    let mut variants = Vec::new();
    for info in registry.all_modules() {
        if &info.path == module_path {
            continue;
        }
        for item in &info.module.items {
            if let ItemKind::Enum(def) = &item.kind
                && def.is_public
            {
                for (idx, variant) in def.variants.iter().enumerate() {
                    let fqn = registry.fqn(
                        &info.path,
                        &[Arc::clone(&def.name), Arc::clone(&variant.name)],
                    );
                    variants.push((
                        fqn,
                        crate::compiler::VariantInfo {
                            enum_name: Arc::clone(&def.name),
                            #[allow(clippy::cast_possible_truncation)]
                            tag: idx as u16,
                            has_payload: variant.payload.is_some(),
                        },
                    ));
                }
            }
        }
    }
    variants
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

    // Compile in dependency order (dependencies first), reusing the same
    // resolve + topo-sort as package modules rather than a hardcoded list.
    // Every core module is registered, so resolving each canonicalizes its
    // cross-module references and yields its dependency set; the ASTs
    // themselves aren't rewritten here (the checker re-resolves, and
    // re-registering would drop the injected intrinsic exports).
    let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut paths_by_key: BTreeMap<String, ModulePath> = BTreeMap::new();
    for core_path in &core_paths {
        // The prelude module is a pure re-export container (`pub use
        // core::Option::{Some, None}`, ...). It is registered so its
        // re-exports can be injected into every scope, but it is never
        // itself checked or compiled: piecewise variant re-exports aren't a
        // normal importable surface, and it contributes no functions. No
        // module ever depends on it (injection resolves to each name's
        // origin), so leaving it out of the order is sound.
        if registry.prelude() == Some(core_path) {
            continue;
        }
        let mut ast = registry
            .get(core_path)
            .map(|info| (*info.module).clone())
            .ok_or_else(|| BuildError::PackageOpen(format!("core module {core_path} vanished")))?;
        let outcome = crate::resolve::resolve_module(&mut ast, core_path, registry);
        deps.insert(
            core_path.to_string(),
            outcome.deps.iter().map(ToString::to_string).collect(),
        );
        paths_by_key.insert(core_path.to_string(), core_path.clone());
    }
    let core_order = compilation_order(&deps);

    let mut merged = CompiledModule::new();
    for core_key in core_order {
        let core_path = paths_by_key
            .get(&core_key)
            .cloned()
            .ok_or_else(|| BuildError::PackageOpen(format!("core module {core_key} vanished")))?;
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

        let compiled = crate::compiler::compile_module_with_options(
            &check_result.module,
            crate::compiler::CompileOptions {
                module_id: Some(registry.module_id(&core_path)),
                imported_hashes: Some(linking_table(module_function_hashes, registry)),
                ..crate::compiler::CompileOptions::default()
            },
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
        // collide with each other or with user functions. Impl-method
        // dispatch symbols are already globally unique and carry their own
        // `::` (`List::all`), so they pass through unqualified — qualifying
        // them again would produce a double-qualified `core::collections::List::all`.
        let mut compiled = compiled;
        compiled.function_names = qualify_names(&compiled.function_names, &core_path, registry);
        compiled.const_names = qualify_names(&compiled.const_names, &core_path, registry);

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
    imported_hashes: HashMap<NameKey, blake3::Hash>,
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
            module_id: Some(registry.module_id(module_path)),
            source: Some(&loaded.source),
            source_file: Some(&source_file),
            imported_hashes: Some(imported_hashes),
            imported_enums: build_imported_enums(module_path, registry),
            imported_unit_structs: build_foreign_unit_structs(module_path, registry),
            imported_const_hashes: build_foreign_const_hashes(module_path, registry),
            foreign_enum_variants: build_foreign_enum_variants(module_path, registry),
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

/// Check and compile a single in-memory session module against a registry,
/// then merge it onto a clone of a cached base module.
///
/// This is the REPL's per-turn pipeline. `base` is the already-built
/// core (+ project) module to merge onto; `imported_hashes` is the matching
/// [`NameKey`] linking table (`CoreContext::hashes` or
/// [`BuildResult::link_table`]) that resolves the session module's
/// cross-module calls. `registry` must already contain `module` (resolved)
/// plus every module it references. The session module's own function names
/// are qualified with `path` (`repl::foo`) before merging — matching how
/// [`build_package`] qualifies package modules — so the caller can deploy an
/// entry by its qualified name (`repl::__repl_entry_N`).
///
/// Mirrors [`compile_loaded_module_with_registry`] but keeps the "how to wire
/// the imported channels" logic here in the engine rather than duplicated in
/// the CLI frontend.
///
/// # Errors
///
/// Returns [`BuildError::TypeCheck`] if the module fails to type-check, or
/// [`BuildError::Compile`] if codegen fails.
#[allow(clippy::implicit_hasher)]
pub fn compile_session_module(
    base: &CompiledModule,
    registry: &ModuleRegistry,
    module: &Module,
    path: &ModulePath,
    source: &str,
    imported_hashes: HashMap<NameKey, blake3::Hash>,
    prelude_abilities: &[Arc<crate::ability_resolver::DynAbility>],
) -> Result<CompiledModule, BuildError> {
    let check_result = crate::infer::check_module_with_registry(module.clone(), path, registry);

    if !check_result.is_ok() {
        return Err(BuildError::TypeCheck {
            module: path.to_string(),
            path: PathBuf::from(path.to_string()),
            source: source.to_string(),
            errors: check_result.errors,
        });
    }

    let source_file = path.to_string();
    let mut compiled = crate::compiler::compile_module_with_options(
        &check_result.module,
        crate::compiler::CompileOptions {
            module_id: Some(registry.module_id(path)),
            source: Some(source),
            source_file: Some(&source_file),
            imported_hashes: Some(imported_hashes),
            imported_enums: build_imported_enums(path, registry),
            imported_unit_structs: build_foreign_unit_structs(path, registry),
            imported_const_hashes: build_foreign_const_hashes(path, registry),
            foreign_enum_variants: build_foreign_enum_variants(path, registry),
            prelude_abilities,
            foreign_abilities: crate::infer::resolve_registry_abilities(registry),
        },
    )
    .map_err(|e| BuildError::Compile {
        module: path.to_string(),
        error: e.to_string(),
    })?;

    // Qualify this module's function and const names with its path
    // (`repl::foo`), the canonical identity, so deploy-by-name resolves them.
    // Impl-method dispatch symbols are already globally unique and pass through.
    compiled.function_names = qualify_names(&compiled.function_names, path, registry);
    compiled.const_names = qualify_names(&compiled.const_names, path, registry);

    let mut merged = base.clone();
    merged.merge(&compiled);
    Ok(merged)
}

/// Qualify a module's bare item names with its path (`gcd` → `math::gcd`),
/// the canonical identity, so store bindings never collide across modules.
/// Names already carrying `::` (impl-method dispatch symbols like
/// `<uuid>::Trait::method`, already globally unique) pass through untouched.
/// Applied identically to function and const name maps.
fn qualify_names(
    names: &HashMap<Arc<str>, blake3::Hash>,
    path: &ModulePath,
    registry: &ModuleRegistry,
) -> HashMap<Arc<str>, blake3::Hash> {
    names
        .iter()
        .map(|(name, hash)| {
            let qualified: Arc<str> = if name.contains("::") {
                Arc::clone(name)
            } else {
                registry.fqn(path, &[Arc::clone(name)]).to_string().into()
            };
            (qualified, *hash)
        })
        .collect()
}

// Tests are in ambient-cli since they require the parser

//! Package building.
//!
//! This is the *single* package pipeline: load every module, register the
//! core/platform/user modules in one registry, canonicalize references
//! (the resolve pass), order modules by their resolved dependencies, and
//! check + compile each one. `ambient run`, `ambient compile`, and
//! `ambient dev` are all frontends over [`build_package`]; behavior that
//! must differ between them is expressed in [`BuildOptions`], never by
//! forking the pipeline.
//!
//! Phase 3 of incremental compilation lives in [`cache`]: when a store path
//! and a prior snapshot are supplied, unchanged modules skip check + compile
//! and load their objects from the store instead. The cold compile machinery
//! is in [`pipeline`].

mod cache;
mod pipeline;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::compiler::{CompiledModule, MigrationRecord};
use crate::fqn::{ModuleId, NameKey};
use crate::infer::BoxedTypeError;
use crate::module_path::ModulePath;
use crate::module_registry::ModuleRegistry;
use crate::package::Package;

pub use cache::{CacheMode, module_cache_key};
pub use pipeline::{
    compile_core_modules, compile_declaration_modules, compile_session_module,
    discover_module_paths, linking_table,
};

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
pub type ParseFn = fn(&str) -> Result<crate::ast::Module, ParseFailure>;

/// Knobs for a package build.
#[derive(Default)]
pub struct BuildOptions<'a> {
    /// The embedder's `core::system` declaration tree (the platform
    /// bindings interface: the directory-module root plus its per-ability
    /// submodules). Empty disables platform registration. Each module
    /// compiles like a core module — its ability method bodies are the
    /// default implementations unhandled performs run — so its `extern fn`
    /// declarations must be bound by [`Self::natives`].
    pub platform_modules: &'a [crate::core_library::DeclModule<'a>],
    /// Embedder native bindings for `extern fn` declarations in the
    /// platform and *user* modules (core's own bindings attach
    /// automatically). The build enforces the full contract: every
    /// declaration bound, every binding declared.
    pub natives: Option<&'a crate::natives::NativeRegistry>,
    /// Optional callback for reporting per-module progress.
    pub progress: Option<ProgressCallback<'a>>,
    /// The package's object store (`<pkg>/.ambient/store`), for incremental
    /// cache hits. `None` disables the cache entirely — a plain cold build.
    /// The build only *reads* the store here; the caller persists the new
    /// build (objects + snapshot) afterwards.
    pub store_path: Option<PathBuf>,
    /// Whether the build may consult the store snapshot ([`CacheMode::Auto`],
    /// the default) or must ignore it ([`CacheMode::Off`]). `AMBIENT_CACHE=off`
    /// forces `Off` regardless.
    pub cache: CacheMode,
}

/// The per-module compile products a build snapshot records: everything
/// keyed to one module that the merged [`CompiledModule`] can no longer
/// attribute back to its source module, plus the incremental-cache metadata
/// (consumed links + cache key). Collected during the per-module compile
/// loops (core, platform, and package) and keyed, like
/// [`BuildResult::interfaces`], by the module's canonical identity string
/// (`core::collections::list`, `workspace::pkg::utils`).
#[derive(Debug, Clone, Default)]
pub struct ModuleBuildOutput {
    /// Canonical object hashes this module produced (redirect stubs
    /// excluded — they are derived from their group), sorted.
    pub objects: Vec<blake3::Hash>,
    /// This module's fully-qualified name → hash bindings (functions and
    /// consts), as the merged store index carries them.
    pub names: BTreeMap<String, blake3::Hash>,
    /// This module's fully-qualified name → canonical signature renderings.
    pub signatures: BTreeMap<String, String>,
    /// The resolve-pass dependency modules, as canonical identity strings.
    pub deps: Vec<String>,
    /// The cross-module link bindings this module consumed, as
    /// `(rendered NameKey, final hash)` pairs sorted by rendering. At a hit,
    /// each must still resolve to the same hash in the current build's
    /// linking state, or the module recompiles (see [`cache`]).
    pub consumed_links: Vec<(String, blake3::Hash)>,
    /// This module's static `State::init_versioned` migration obligations.
    pub migrations: Vec<MigrationRecord>,
    /// This module's lambda hash → parent name entries, sorted by hash.
    pub lambda_parents: Vec<(blake3::Hash, String)>,
    /// This module's entry point (`run`), if it declares one.
    pub entry_point: Option<blake3::Hash>,
    /// This module's incremental-cache key. Zero for builtin (core/platform)
    /// modules, which cache as one unit keyed separately.
    pub cache_key: [u8; 32],
}

/// Result of building a package.
pub struct BuildResult {
    /// The compiled module containing all functions.
    pub compiled: CompiledModule,
    /// Number of package modules in the build (compiled or cache-loaded).
    pub module_count: usize,
    /// Number of modules actually check+compiled this build (i.e. cache
    /// *misses*, plus every module of a cold builtin block). Zero on a fully
    /// warm build. Instrumentation for the incremental-cache tests.
    pub modules_compiled: usize,
    /// Package name.
    pub package_name: String,
    /// The canonical [`NameKey`] linking table for the whole build (core +
    /// package). Consumers that compile *additional* modules against this
    /// build — the REPL, notably — pass it as `imported_hashes`.
    pub link_table: HashMap<NameKey, blake3::Hash>,
    /// The content-keyed interface of every registered module (core,
    /// platform, and package), keyed by the module's canonical identity
    /// string. Computed from the resolved ASTs.
    pub interfaces: BTreeMap<String, crate::module_interface::ModuleInterfaceSummary>,
    /// The build-global dispatch-surface hash: a fold of every module's
    /// impl + ability sections (the coherence/dispatch channel).
    pub dispatch_surface_hash: blake3::Hash,
    /// Per-module compile products (objects, name bindings, signatures,
    /// dependency sets, consumed links, migrations, …), keyed like
    /// [`Self::interfaces`]. The persisted build manifest folds these
    /// together with each module's interface.
    pub module_outputs: BTreeMap<String, ModuleBuildOutput>,
    /// A deterministic hash of the whole native-binding surface the build
    /// saw (core plus embedder), from
    /// [`NativeRegistry::contract_hash`](crate::natives::NativeRegistry::contract_hash).
    /// The manifest records it so a drifted host table is a cache miss.
    pub natives_contract_hash: blake3::Hash,
    /// The core+platform unit cache key this build computed. The manifest
    /// records it so the next build can load the whole builtin block on a hit.
    pub core_cache_key: [u8; 32],
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
    /// The package's modules form an import cycle. The module dependency
    /// graph is a hard DAG (see [`crate::module_cycles`]); the message is the
    /// canonical rendering the analysis pipeline reports too. Spanless: the
    /// cycle is a package-structural fact, not a single-site error.
    ImportCycle { message: String },
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
            Self::ImportCycle { message } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for BuildError {}

/// Build an Ambient package.
///
/// Pipeline:
/// 1. Load and parse every `.ab` file under `src/`.
/// 2. Register core modules, the `core::system` declaration module, and every
///    package module in one [`ModuleRegistry`]. Core + platform either compile
///    or, on a cache hit, load their objects from the store.
/// 3. Run the resolve pass over each package module: canonicalize every
///    cross-module reference and collect the true dependency graph.
/// 4. For each package module in dependency order: compute its cache key; on a
///    validated hit load its objects from the store, else check + compile.
///
/// # Errors
///
/// Returns an error if the package can't be opened, a module fails to parse or
/// type-check, or compilation fails.
///
/// # Panics
///
/// Under `AMBIENT_CACHE_VERIFY=1`, panics if a cache hit disagrees with a
/// fresh recompile — the standing under-invalidation oracle. Never panics in
/// ordinary builds.
#[allow(clippy::arc_with_non_send_sync, clippy::too_many_lines)]
pub fn build_package(
    path: &Path,
    parse: ParseFn,
    options: &BuildOptions<'_>,
) -> Result<BuildResult, BuildError> {
    let mut pkg = Package::open(path).map_err(|e| BuildError::PackageOpen(e.to_string()))?;
    let package_name = pkg.manifest().name.clone();

    // Loading everything (rather than chasing `use` from `main`) is what makes
    // directory namespaces and inline `pkg::a::b::f()` work: the module graph
    // is the filesystem, the dependency graph the resolve pass below.
    pipeline::load_all_modules(&mut pkg, parse)?;

    let mut registry = ModuleRegistry::new();
    // Scope every user item's `Fqn` under the package name; core is `Builtin`.
    registry.set_workspace_name(package_name.clone());

    // Core registration only needs a string on failure (a parse error there is
    // a compiler bug, not user error), so adapt the richer `ParseFn`.
    let parse_str = |s: &str| parse(s).map_err(|e| e.message);

    // Attach embedder native bindings before anything compiles (core's own
    // bindings attach inside `register_core_modules`).
    if let Some(natives) = options.natives {
        registry.natives_mut().merge(natives);
    }

    let cache = cache::BuildCache::open(options.store_path.as_deref(), options.cache);

    // ── Builtins (core + platform): register always, load-or-compile. ──
    // Registration must happen every build — the registry needs the ASTs for
    // foreign-signature hydration and interface derivation — but check +
    // compile can be skipped when the builtin unit key matches.
    let core_paths = crate::core_library::register_core_modules(&mut registry, parse_str).map_err(
        |(module, error)| BuildError::Compile {
            module: format!("core.{module}"),
            error,
        },
    )?;
    let platform_paths = if options.platform_modules.is_empty() {
        Vec::new()
    } else {
        crate::core_library::register_declaration_modules(
            &mut registry,
            options.platform_modules,
            parse_str,
        )
        .map_err(|(module, error)| BuildError::Compile { module, error })?
    };
    let builtin_paths: Vec<ModulePath> =
        core_paths.iter().chain(&platform_paths).cloned().collect();

    // The full native surface the build saw: core's own bindings plus any
    // embedder bindings, all merged into the registry above.
    let natives_contract_hash = registry.natives().contract_hash();
    let core_key = cache::core_cache_key(&registry, &builtin_paths, natives_contract_hash);

    let mut all_compiled = CompiledModule::new();
    let mut module_function_hashes: HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>> =
        HashMap::new();
    let mut module_outputs: BTreeMap<String, ModuleBuildOutput> = BTreeMap::new();
    let mut modules_compiled = 0usize;

    let builtins_loaded = cache.core_key_matches(core_key)
        && cache.load_builtins(
            &registry,
            &builtin_paths,
            &mut all_compiled,
            &mut module_function_hashes,
            &mut module_outputs,
        );
    if !builtins_loaded {
        let before = module_outputs.len();
        let core_compiled = pipeline::compile_module_group(
            &mut registry,
            &mut module_function_hashes,
            &mut module_outputs,
            &core_paths,
        )?;
        all_compiled.merge(&core_compiled);
        if !platform_paths.is_empty() {
            let platform_compiled = pipeline::compile_module_group(
                &mut registry,
                &mut module_function_hashes,
                &mut module_outputs,
                &platform_paths,
            )?;
            all_compiled.merge(&platform_compiled);
        }
        modules_compiled += module_outputs.len() - before;
    }

    // ── Package modules: register raw, resolve, re-register resolved. ──
    for module in pkg.all_modules() {
        registry.register(&module.path, Arc::new(module.ast.clone()));
    }
    let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut paths_by_key: BTreeMap<String, ModulePath> = BTreeMap::new();
    // The resolve-pass dependency sets, keyed and rendered by canonical module
    // identity (matching `interfaces`/`module_outputs` keys) so cache keys can
    // fold each dependency's interface hash.
    let mut dep_ids: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // The raw resolve-pass dependency closures, keyed by module identity, for
    // narrowing each module's `ModuleEnv` to what it can actually reference.
    let mut dep_closures: BTreeMap<String, std::collections::BTreeSet<ModuleId>> = BTreeMap::new();
    for module in pkg.all_modules_mut() {
        let outcome = crate::resolve::resolve_module(&mut module.ast, &module.path, &registry);
        deps.insert(
            module.path.to_string(),
            outcome
                .deps
                .iter()
                .map(ModuleId::module_path_string)
                .collect(),
        );
        dep_ids.insert(
            registry.module_id(&module.path).to_string(),
            outcome.deps.iter().map(ToString::to_string).collect(),
        );
        dep_closures.insert(registry.module_id(&module.path).to_string(), outcome.deps);
        paths_by_key.insert(module.path.to_string(), module.path.clone());
    }
    for module in pkg.all_modules() {
        registry.register(&module.path, Arc::new(module.ast.clone()));
    }

    // The module dependency graph is a hard DAG: reject import cycles with a
    // clear diagnostic (the analysis pipeline reports the same rendering).
    if let Some(cycle) = crate::module_cycles::detect_import_cycles(&deps).first() {
        return Err(BuildError::ImportCycle {
            message: cycle.describe(),
        });
    }

    // Enforce the extern-fn contract in both directions before compiling.
    let violations = registry.verify_native_contract();
    if !violations.is_empty() {
        let joined = violations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(BuildError::Compile {
            module: "extern bindings".to_string(),
            error: joined,
        });
    }

    // Interfaces + dispatch surface: computed before the compile loop because
    // each package module's cache key folds the dispatch-surface hash and its
    // dependencies' interface hashes.
    let interfaces = crate::module_interface::build_interfaces(&registry);
    let dispatch_surface_hash = crate::module_interface::dispatch_surface_hash(&interfaces);
    let dispatch_bytes = *dispatch_surface_hash.as_bytes();
    let natives_bytes = *natives_contract_hash.as_bytes();

    // The incremental linking state, seeded from the builtin block and then
    // extended once per package module — never rebuilt from scratch.
    let mut link = cache::LinkState::default();
    link.seed(&module_function_hashes, &registry);

    // Compile (or load) package modules in dependency order.
    let module_order = pipeline::compilation_order(&deps);
    let total_modules = module_order.len();

    for (idx, module_key) in module_order.iter().enumerate() {
        let module_path = paths_by_key
            .get(module_key)
            .cloned()
            .ok_or_else(|| BuildError::PackageOpen(format!("module not found: {module_key}")))?;
        let module_id = registry.module_id(&module_path).to_string();

        if let Some(ref cb) = options.progress {
            cb(module_key, idx + 1, total_modules);
        }

        // This module's cache key (None only if a dependency interface is
        // somehow absent — then it can never hit and always recompiles).
        let cache_key = dep_interface_hashes(&dep_ids, &module_id, &interfaces).map(|deps| {
            let ast = interfaces
                .get(&module_id)
                .map_or([0u8; 32], |s| *s.resolved_ast_hash.as_bytes());
            cache::module_cache_key(ast, natives_bytes, dispatch_bytes, &deps)
        });

        let imported = link.table();
        let hit = cache_key.and_then(|k| cache.try_package_module(&module_id, k, &link));

        let (compiled, output) = match hit {
            // A validated hit and no verify oracle: use the loaded module.
            Some((loaded, loaded_output)) if !cache.verify() => (loaded, loaded_output),
            // Miss, or verify mode (recompile and compare against the hit).
            hit => {
                modules_compiled += 1;
                let module = pkg.get_module(&module_path).ok_or_else(|| {
                    BuildError::PackageOpen(format!("module not found: {module_path}"))
                })?;
                let file_path = pkg.module_file_path(&module_path);
                let mut compiled = pipeline::compile_loaded_module_with_registry(
                    module,
                    &file_path,
                    &module_path,
                    &registry,
                    imported.clone(),
                    dep_closures
                        .get(&module_id)
                        .unwrap_or(&std::collections::BTreeSet::new()),
                )?;
                // Qualify this module's bare names with its path (`math::gcd`),
                // the canonical identity, matching the merged store index.
                compiled.function_names =
                    pipeline::qualify_names(&compiled.function_names, &module_path, &registry);
                compiled.const_names =
                    pipeline::qualify_names(&compiled.const_names, &module_path, &registry);
                compiled.signatures =
                    pipeline::qualify_names(&compiled.signatures, &module_path, &registry);
                let output = cache::module_output(
                    &compiled,
                    dep_ids.get(&module_id).cloned().unwrap_or_default(),
                    &imported,
                    cache_key.unwrap_or([0u8; 32]),
                );
                // Verify oracle: a hit that disagrees with the fresh compile is
                // an under-invalidation bug — fail loudly with a precise diff.
                if let Some((_, loaded_output)) = hit
                    && let Err(msg) = cache::assert_equivalent(&module_id, &loaded_output, &output)
                {
                    panic!("{msg}");
                }
                (compiled, output)
            }
        };

        link.extend_module(
            &compiled.function_names,
            &module_id,
            &module_path,
            &registry,
        );
        module_outputs.insert(module_id, output);
        all_compiled.merge(&compiled);
    }

    Ok(BuildResult {
        compiled: all_compiled,
        module_count: total_modules,
        modules_compiled,
        package_name,
        link_table: link.into_table(),
        interfaces,
        dispatch_surface_hash,
        module_outputs,
        natives_contract_hash,
        core_cache_key: core_key,
    })
}

/// A package module's sorted `(dependency id, interface hash)` list, for its
/// cache key. Returns `None` if any dependency has no interface summary (which
/// would make the key unstable) — the module then can never hit.
///
/// Public so the analysis pipeline keys its check memo from the identical
/// derivation (see [`module_cache_key`]).
#[must_use]
pub fn dep_interface_hashes(
    dep_ids: &BTreeMap<String, Vec<String>>,
    module_id: &str,
    interfaces: &BTreeMap<String, crate::module_interface::ModuleInterfaceSummary>,
) -> Option<Vec<(String, [u8; 32])>> {
    let mut deps: Vec<String> = dep_ids.get(module_id).cloned().unwrap_or_default();
    deps.sort_unstable();
    deps.dedup();
    let mut out = Vec::with_capacity(deps.len());
    for dep in deps {
        let iface = interfaces.get(&dep)?;
        out.push((dep, *iface.interface_hash.as_bytes()));
    }
    Some(out)
}

// Tests are in ambient-cli since they require the parser.

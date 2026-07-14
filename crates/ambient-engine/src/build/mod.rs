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
mod check_prepass;
mod dispatch_scope;
mod pipeline;
mod reachability;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::compiler::{CompiledModule, MigrationRecord};
use crate::disk_store::{BuildManifest, DiskStore, DiskStoreError};
use crate::fqn::{ModuleId, NameKey};
use crate::infer::BoxedTypeError;
use crate::module_path::ModulePath;
use crate::module_registry::ModuleRegistry;
use crate::package::Package;

pub use cache::{CacheMode, module_cache_key};
pub use dispatch_scope::per_module_dispatch_hashes;
pub use pipeline::{
    compile_core_modules, compile_declaration_modules, compile_session_module,
    discover_module_paths, linking_table,
};

/// Progress callback for reporting build progress.
///
/// Called with `(module name, current, total, from_cache)` for each package
/// module, where `from_cache` is `true` when the module was loaded from the
/// store instead of check+compiled (a cache hit). The verify oracle recompiles
/// every module, so under it `from_cache` is always `false` — the callback
/// reports what actually happened, not merely that a hit was available.
pub type ProgressCallback<'a> = &'a dyn Fn(&str, usize, usize, bool);

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
    /// The entry function to build for, when the build should be **lazy**:
    /// compile only the package modules reachable from that entry (see
    /// [`reachability`]). `None` (the default) builds the whole package —
    /// what `ambient check`, `ambient compile`, and `ambient dev` want. Only
    /// `ambient run` sets this. If no package module declares a matching
    /// entry function, the build silently falls back to whole-package.
    pub entry: Option<&'a str>,
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
    /// The content hash of this module's persisted pre-link symbolic form, if
    /// it has one (the relink fast path's input). `None` for builtin modules.
    pub prelink: Option<blake3::Hash>,
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
    /// Number of package modules **type-checked** this build: the check
    /// pre-pass's cache-missing modules plus any walk-time lazy fallback check
    /// (verify mode, or a key match that fails both hit and relink). Zero on a
    /// fully warm build — a hit/relink never re-checks. Instrumentation pinning
    /// the pre-pass property that a warm build performs no checks. Excludes the
    /// builtin block (cached as one unit).
    pub modules_checked: usize,
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
    /// Fresh pre-link blobs produced this build (by compiled *and* relinked
    /// modules), keyed by content hash. [`persist_build`] writes these to the
    /// store's `prelink/` area before the snapshot pointer flips, so a rooted
    /// manifest's prelink references always resolve. A cache *hit* re-uses the
    /// prior blob already in the store and contributes nothing here.
    pub prelink_blobs: BTreeMap<[u8; 32], Vec<u8>>,
    /// Number of modules served by the relink fast path this build (key match,
    /// link-only miss, remapped and re-finalized without re-check/codegen).
    /// Instrumentation for the incremental-cache tests.
    pub modules_relinked: usize,
}

/// One module's type-check failure: the offending module's identity, source,
/// and file path alongside its structured (spanned) errors, so a frontend can
/// render them with source context — byte-identically to `ambient check`.
///
/// The check pre-pass ([`check_prepass`]) checks every cache-missing module up
/// front, so a single cold build can fail in several modules at once; each is
/// captured here and they surface together (see [`BuildError::TypeCheck`]).
#[derive(Debug)]
pub struct ModuleTypeErrors {
    /// The module's canonical identity (for messages).
    pub module: String,
    /// The module's real on-disk source path, for rendering.
    pub path: PathBuf,
    /// The module's full source text, for rendering source context.
    pub source: String,
    /// The structured, spanned type errors.
    pub errors: Vec<BoxedTypeError>,
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
    /// One or more user modules failed to type-check. The check pre-pass
    /// aggregates every cache-missing module's failure so a cold build reports
    /// them all at once (deterministically ordered by module identity); the
    /// compile-time fallback check contributes a single-element vec.
    TypeCheck { failures: Vec<ModuleTypeErrors> },
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
            Self::TypeCheck { failures } => {
                let joined = failures
                    .iter()
                    .map(|failure| {
                        let errs = failure
                            .errors
                            .iter()
                            .map(|e| e.kind.to_string())
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("type errors in {}: {errs}", failure.module)
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                write!(f, "{joined}")
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
    let mut modules_relinked = 0usize;
    // Fresh pre-link blobs (compiled + relinked modules), persisted before the
    // snapshot pointer flips.
    let mut prelink_blobs: BTreeMap<[u8; 32], Vec<u8>> = BTreeMap::new();

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

    // Re-register the builtin ASTs in their resolved form, unconditionally
    // (a cache hit above skips their compile — and thus their resolve — so this
    // is the only place the loaded-from-store path canonicalizes them). This is
    // what makes builtin interface derivation and foreign-signature hydration
    // read one AST form, matching package modules and `ambient-analysis`; the
    // builtin objects were already compiled/loaded above and are untouched.
    crate::core_library::resolve_builtin_modules(&mut registry, &builtin_paths);

    // ── Package modules: register raw, resolve, re-register resolved. ──
    for module in pkg.all_modules() {
        registry.register(&module.path, Arc::new(module.ast.clone()));
        // Record the real on-disk path so the snapshot manifest resolves a
        // directory module to its `<dir>/main.ab` rather than reconstructing a
        // nonexistent `<dir>.ab`. `register` preserves it across the resolved
        // re-registration below, so it only needs setting once.
        if let Some(source_path) = &module.source_path {
            registry.set_source_path(&module.path, source_path.clone());
        }
    }
    let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // The link-order subset of `deps` (value/symbol-position references only),
    // keyed the same way (dotted `ModulePath`). This is the base of the
    // compile-ordering graph: only link-time edges must constrain compile order,
    // and dropping the check-order-only edges is what lets the self-orphan
    // dispatch cycle link. Every module is present as a key (possibly empty) so
    // `compilation_order` sees the whole graph. See [`reachability`].
    let mut link_deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
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
        link_deps.insert(
            module.path.to_string(),
            outcome
                .link_deps
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
    // The build-global dispatch surface is retained for the manifest/`BuildResult`
    // (informational + the whole-build interface tests), but the per-module cache
    // key now folds a *narrowed* dispatch input: only the impl shapes each module
    // can dispatch (plus the global unconditional/colliding/ability bytes). See
    // [`dispatch_scope`].
    let dispatch_surface_hash = crate::module_interface::dispatch_surface_hash(&interfaces);
    let module_dispatch = dispatch_scope::per_module_dispatch_hashes(&registry, &dep_ids);
    let natives_bytes = *natives_contract_hash.as_bytes();

    // The incremental linking state, seeded from the builtin block and then
    // extended once per package module — never rebuilt from scratch.
    let mut link = cache::LinkState::default();
    link.seed(&module_function_hashes, &registry);

    // Compile (or load) package modules in dependency order.
    //
    // A lazy build (`ambient run`, `options.entry` set) restricts this to the
    // modules reachable from the entry — filtering the whole-package order
    // rather than recomputing it, so a reached module compiles in the exact
    // relative order (and against the exact accumulated linking state) it would
    // in a full build, keeping its objects byte-identical. Unreached modules
    // are never checked, so their diagnostics are (by policy) not reported by
    // `run`. See [`reachability`] and `ref/modules.md`.
    // Order by *link* deps plus structural type-directed dispatch edges, so an
    // orphan trait impl compiles before any module that dispatches it — even one
    // the dispatcher never imports, one that sorts after it alphabetically, or
    // the type's own module (the self-orphan case). Using `link_deps` (not the
    // full `deps`) as the base is what makes the self-orphan case acyclic: a
    // `use`/type-target edge back to the type's module is check-order-only and
    // must not constrain compile order. See [`reachability::dispatch_ordering_graph`]
    // (per-edge acyclic augmentation: a structural edge is added only when it
    // keeps the graph acyclic, so a genuinely cyclic dispatch dep drops its own
    // edge and fails to link exactly as before — correct — without poisoning
    // unrelated edges).
    let ordering_modules: Vec<(String, &crate::ast::Module)> = pkg
        .all_modules()
        .map(|m| (m.path.to_string(), &m.ast))
        .collect();
    let ordering_graph =
        reachability::dispatch_ordering_graph(&deps, &link_deps, &ordering_modules);
    let full_order = pipeline::compilation_order(&ordering_graph);
    let reachable = options.entry.and_then(|entry| {
        let modules: Vec<reachability::PackageModule<'_>> = pkg
            .all_modules()
            .map(|m| reachability::PackageModule {
                id: registry.module_id(&m.path).to_string(),
                ast: &m.ast,
            })
            .collect();
        reachability::reachable_module_ids(entry, &dep_ids, &modules)
    });
    let module_order: Vec<String> = match &reachable {
        Some(reachable) => full_order
            .into_iter()
            .filter(|key| {
                paths_by_key
                    .get(key)
                    .is_some_and(|path| reachable.contains(&registry.module_id(path).to_string()))
            })
            .collect(),
        None => full_order,
    };
    let total_modules = module_order.len();

    // Every module's cache key, computed once up front (the key never depends
    // on `LinkState`), keyed by canonical module id. The check pre-pass and the
    // compile walk both read this map — the key is never recomputed.
    let cache_keys = check_prepass::compute_cache_keys(
        &module_order,
        &paths_by_key,
        &registry,
        &dep_ids,
        &interfaces,
        &module_dispatch,
        natives_bytes,
    );

    // Check pre-pass: type-check every cache-*missing* module up front, in a
    // deterministic order, so a cold build surfaces *all* modules' check errors
    // together (not just the first in compile order). Key-match modules are not
    // checked here — a warm hit or relink must never re-check; the rare
    // recompile fallbacks (verify mode, unlinkable key match) check lazily in
    // the walk. Checking is globally order-independent, so this changes only
    // *when* checks run, never their outcome. See [`check_prepass`].
    let mut checked = check_prepass::run(
        &pkg,
        &registry,
        &cache,
        &module_order,
        &paths_by_key,
        &cache_keys,
    )?;
    // Every pre-pass check, plus any walk-time lazy fallback check tallied below.
    let mut modules_checked = checked.len();

    for (idx, module_key) in module_order.iter().enumerate() {
        let module_path = paths_by_key
            .get(module_key)
            .cloned()
            .ok_or_else(|| BuildError::PackageOpen(format!("module not found: {module_key}")))?;
        let module_id = registry.module_id(&module_path).to_string();

        // This module's cache key, precomputed above (None only if a dependency
        // interface is somehow absent — then it can never hit and always
        // recompiles).
        let cache_key = cache_keys.get(&module_id).copied().flatten();

        let imported = link.table();
        let key = cache_key.unwrap_or([0u8; 32]);
        let deps_list = dep_ids.get(&module_id).cloned().unwrap_or_default();

        // A full hit: key matches and every consumed link still resolves to the
        // same hash. Failing that, the relink fast path: the key still matches
        // (check output unchanged) but a dependency's body moved a callee hash,
        // so remap the moved foreign hashes and re-finalize — no re-check, no
        // codegen. Any relink obstacle (or an assembly failure the recompile
        // would report properly) falls back to a full compile.
        let hit = cache_key.and_then(|k| cache.try_package_module(&module_id, k, &link));
        let relink = if hit.is_none() {
            cache_key
                .and_then(|k| cache.try_relink_module(&module_id, k, &link))
                .and_then(|prelink| {
                    let mut compiled =
                        crate::compiler::assemble_module(prelink.to_assemble_inputs()).ok()?;
                    compiled.signatures = prelink.signature_map();
                    Some((compiled, prelink))
                })
        } else {
            None
        };

        // Report progress once the outcome is known: `from_cache` marks a
        // module served without check+compile (a full hit or a relink). The
        // verify oracle recompiles regardless, so it reports what happened.
        if let Some(ref cb) = options.progress {
            cb(
                module_key,
                idx + 1,
                total_modules,
                (hit.is_some() || relink.is_some()) && !cache.verify(),
            );
        }

        let (compiled, output) = if cache.verify() {
            // Verify oracle: always recompile, then assert every available warm
            // path (a full hit and/or a relink) is byte-identical to it.
            modules_compiled += 1;
            // A key-match module reaches verify mode without a pre-pass check;
            // it is checked lazily here (miss modules carry their pre-pass one).
            let pre_checked = checked.remove(&module_id);
            if pre_checked.is_none() {
                modules_checked += 1;
            }
            let (bare, prelink) = compile_package_module(
                &pkg,
                &registry,
                &module_path,
                &imported,
                dep_closures
                    .get(&module_id)
                    .unwrap_or(&std::collections::BTreeSet::new()),
                pre_checked,
            )?;
            let (compiled, output, blob) = finish_module(
                bare,
                &prelink,
                &module_path,
                &registry,
                deps_list.clone(),
                &imported,
                key,
            );
            if let Some((_, loaded_output)) = &hit
                && let Err(msg) = cache::assert_equivalent(&module_id, loaded_output, &output)
            {
                panic!("{msg}");
            }
            if let Some((relink_bare, relink_prelink)) = &relink {
                let (_, relink_output, _) = finish_module(
                    relink_bare.clone(),
                    relink_prelink,
                    &module_path,
                    &registry,
                    deps_list,
                    &imported,
                    key,
                );
                if let Err(msg) = cache::assert_equivalent(&module_id, &relink_output, &output) {
                    panic!("{msg}");
                }
            }
            insert_blob(&mut prelink_blobs, blob);
            (compiled, output)
        } else if let Some((loaded, loaded_output)) = hit {
            // A validated hit: use the loaded module (its prelink blob is
            // already durable from the build that produced it).
            (loaded, loaded_output)
        } else if let Some((relink_bare, relink_prelink)) = relink {
            modules_relinked += 1;
            let (compiled, output, blob) = finish_module(
                relink_bare,
                &relink_prelink,
                &module_path,
                &registry,
                deps_list,
                &imported,
                key,
            );
            insert_blob(&mut prelink_blobs, blob);
            (compiled, output)
        } else {
            modules_compiled += 1;
            // No pre-pass check only on the rare key-match-but-unlinkable
            // fallback (hit and relink both failed); check lazily then.
            let pre_checked = checked.remove(&module_id);
            if pre_checked.is_none() {
                modules_checked += 1;
            }
            let (bare, prelink) = compile_package_module(
                &pkg,
                &registry,
                &module_path,
                &imported,
                dep_closures
                    .get(&module_id)
                    .unwrap_or(&std::collections::BTreeSet::new()),
                pre_checked,
            )?;
            let (compiled, output, blob) = finish_module(
                bare,
                &prelink,
                &module_path,
                &registry,
                deps_list,
                &imported,
                key,
            );
            insert_blob(&mut prelink_blobs, blob);
            (compiled, output)
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

    // The walk consumes each pre-pass check exactly once (`checked.remove` on
    // every compile path). A leftover means the pre-pass's cache predicate
    // (`check_prepass`'s `key_matches`) and the walk's predicates
    // (`try_package_module` / `try_relink_module`) disagreed about which
    // modules hit — a drift that would silently violate checked-exactly-once
    // (a module checked up front but recompiled, or vice versa). Trip it here.
    debug_assert!(
        checked.is_empty(),
        "pre-pass checks left unconsumed after the compile walk (cache-predicate drift): {:?}",
        checked.keys().collect::<Vec<_>>(),
    );

    Ok(BuildResult {
        compiled: all_compiled,
        module_count: total_modules,
        modules_compiled,
        modules_checked,
        package_name,
        link_table: link.into_table(),
        interfaces,
        dispatch_surface_hash,
        module_outputs,
        natives_contract_hash,
        core_cache_key: core_key,
        prelink_blobs,
        modules_relinked,
    })
}

/// Compile one package module, capturing its pre-link symbolic form. The
/// returned module has bare names (the caller qualifies via [`finish_module`]);
/// its `signatures` and the prelink's are already set.
///
/// `checked` is the module's pre-pass check result when it was checked up front
/// (the ordinary miss path); `None` triggers a lazy check here — the only two
/// paths that reach compilation without a pre-pass check: verify mode
/// (recompiles even on a key match) and the rare key-match-but-unlinkable
/// fallback. A module is thus checked exactly once per build.
fn compile_package_module(
    pkg: &Package,
    registry: &ModuleRegistry,
    module_path: &ModulePath,
    imported: &HashMap<NameKey, blake3::Hash>,
    deps: &std::collections::BTreeSet<ModuleId>,
    checked: Option<crate::infer::CheckResult>,
) -> Result<(CompiledModule, crate::compiler::PrelinkModule), BuildError> {
    let module = pkg
        .get_module(module_path)
        .ok_or_else(|| BuildError::PackageOpen(format!("module not found: {module_path}")))?;
    // Prefer the real discovered path (a directory module's `<dir>/main.ab`)
    // for diagnostics; fall back to the canonical reconstruction only for a
    // module with no recorded on-disk path.
    let file_path = module.source_path.as_ref().map_or_else(
        || pkg.module_file_path(module_path),
        |sp| pkg.src_path().join(sp),
    );
    let check_result = match checked {
        Some(cr) => cr,
        None => pipeline::check_loaded_module(module, &file_path, module_path, registry)?,
    };
    pipeline::compile_checked_module(
        module,
        &file_path,
        module_path,
        registry,
        imported.clone(),
        deps,
        check_result,
    )
}

/// Qualify a freshly compiled or relinked module's names, encode its pre-link
/// blob, and derive its [`ModuleBuildOutput`] (with the blob's hash recorded).
/// A prelink that cannot be encoded yields `None` — the module simply has no
/// relink input next time (a safe, slower fallback), never a build error.
fn finish_module(
    mut compiled: CompiledModule,
    prelink: &crate::compiler::PrelinkModule,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
    deps: Vec<String>,
    imported: &HashMap<NameKey, blake3::Hash>,
    cache_key: [u8; 32],
) -> (
    CompiledModule,
    ModuleBuildOutput,
    Option<(blake3::Hash, Vec<u8>)>,
) {
    compiled.function_names =
        pipeline::qualify_names(&compiled.function_names, module_path, registry);
    compiled.const_names = pipeline::qualify_names(&compiled.const_names, module_path, registry);
    compiled.signatures = pipeline::qualify_names(&compiled.signatures, module_path, registry);
    let blob = prelink
        .encode()
        .ok()
        .map(|bytes| (blake3::hash(&bytes), bytes));
    let mut output = cache::module_output(&compiled, deps, imported, cache_key);
    output.prelink = blob.as_ref().map(|(h, _)| *h);
    (compiled, output, blob)
}

/// Record a fresh pre-link blob for persistence, if the module produced one.
fn insert_blob(blobs: &mut BTreeMap<[u8; 32], Vec<u8>>, blob: Option<(blake3::Hash, Vec<u8>)>) {
    if let Some((hash, bytes)) = blob {
        blobs.insert(*hash.as_bytes(), bytes);
    }
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

/// A completed build paired with the outcome of persisting it to the
/// package-local store.
///
/// Persisting is best-effort: the store is a rebuildable content-addressed
/// cache, so a persist failure never fails the build. Callers decide how to
/// react to [`Self::persisted`] — the CLI warns and continues, tests assert it
/// succeeded.
pub struct PersistedBuild {
    /// The build result.
    pub result: BuildResult,
    /// Whether the objects + snapshot were durably written. `Err` carries the
    /// store failure (opening the store or writing it).
    pub persisted: Result<(), DiskStoreError>,
}

/// Persist a completed build to a store: objects and name bindings first, then
/// the crash-safe snapshot. Ordering matters — the snapshot's root pointer is
/// only swapped after every object it references and the manifest bytes are
/// durably in place ([`DiskStore::write_snapshot`]), so a snapshot always
/// resolves to a consistent build.
///
/// # Errors
///
/// Returns any store write failure.
pub fn persist_build(disk: &DiskStore, result: &BuildResult) -> Result<(), DiskStoreError> {
    disk.put_module(&result.compiled)?;
    // Pre-link blobs before the snapshot: the manifest's `prelink` references
    // must be durable before the root pointer flips (same crash-safety ordering
    // as objects), so a rooted snapshot's relink inputs always resolve.
    for bytes in result.prelink_blobs.values() {
        disk.put_prelink(bytes)?;
    }
    let manifest = BuildManifest::from_build(result);
    disk.write_snapshot(&manifest)?;
    Ok(())
}

/// Build a package and persist it to its package-local content-addressed store
/// so the next build is warm.
///
/// This is the single wiring `ambient run`, `ambient compile`, and the
/// incremental-cache tests share: the build reads the prior snapshot from the
/// same store this then persists to, so `store_path` is derived here (from
/// `path`) rather than by each caller — any `store_path` on `options` is
/// overwritten. Persisting is best-effort; see [`PersistedBuild`].
///
/// # Errors
///
/// Returns a [`BuildError`] if the build itself fails. A persist failure is
/// reported in [`PersistedBuild::persisted`], not here.
pub fn build_and_persist(
    path: &Path,
    parse: ParseFn,
    mut options: BuildOptions<'_>,
) -> Result<PersistedBuild, BuildError> {
    options.store_path = Some(DiskStore::package_store_path(path));
    let result = build_package(path, parse, &options)?;
    let persisted = match DiskStore::open_package(path) {
        Ok(disk) => persist_build(&disk, &result),
        Err(e) => Err(e),
    };
    Ok(PersistedBuild { result, persisted })
}

/// Build a package **lazily** for `ambient run`: compile only the modules
/// reachable from `entry` (see [`reachability`]), reading the package-local
/// store for warm cache hits but writing **no** snapshot.
///
/// The read-only-cache policy is deliberate and is the simplest sound snapshot
/// semantics for a partial build: a lazy build never checks (and so never
/// records) the unreached modules, so persisting its manifest would either
/// strand ghost records or, if partial, mislead `store diff` (which computes
/// removals) and the store gc (whose roots are the snapshot's referenced
/// objects). By only reading, a lazy run fully exploits a warm snapshot that a
/// prior whole-package `ambient compile`/`dev` (the snapshot writers) left, yet
/// can never corrupt one. The reached objects it produces are byte-identical to
/// a full build's, so nothing is lost but the warming of the store from `run`
/// itself. See `ref/modules.md` ("Lazy compilation").
///
/// # Errors
///
/// Returns a [`BuildError`] if the build fails.
pub fn build_reachable<'a>(
    path: &Path,
    parse: ParseFn,
    mut options: BuildOptions<'a>,
    entry: &'a str,
) -> Result<BuildResult, BuildError> {
    options.store_path = Some(DiskStore::package_store_path(path));
    options.entry = Some(entry);
    build_package(path, parse, &options)
}

// Tests are in ambient-cli since they require the parser.

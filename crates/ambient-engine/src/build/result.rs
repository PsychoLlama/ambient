//! Build inputs and outputs: the option knobs a build takes and the result and
//! error types it produces.
//!
//! Pure data types shared across the pipeline and re-exported from
//! [`super`](crate::build) so consumers keep their `crate::build::…` paths.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use crate::compiler::{CompiledModule, MigrationRecord};
use crate::fqn::NameKey;
use crate::infer::BoxedTypeError;

use super::CacheMode;

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
    /// [`reachability`](super::reachability)). `None` (the default) builds the
    /// whole package — what `ambient check`, `ambient build`, and `ambient
    /// dev` want. Only `ambient run` sets this. If no package module declares a
    /// matching entry function, the build silently falls back to whole-package.
    pub entry: Option<&'a str>,
    /// Narrow the build's **target** to one workspace package by name (the
    /// CLI's `--package`). Every member still loads and registers — laziness
    /// is only about what gets checked and compiled. `None` targets whatever
    /// the build path implies (a member dir → that member; the workspace
    /// root → all members).
    pub package: Option<&'a str>,
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
    /// linking state, or the module recompiles (see [`cache`](super::cache)).
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
    ///
    /// Identity: `modules_checked == modules_compiled - <cold builtin-block
    /// count>` — every recompiled package module is checked exactly once
    /// (pre-pass hit or walk-time fallback), so the two move in lockstep.
    pub modules_checked: usize,
    /// Primary package name (the first build target) — display shorthand.
    pub package_name: String,
    /// Every **target** package this build covers (one for a standalone or
    /// member build; all members for a workspace-root build). The persist
    /// layer writes one snapshot pointer per entry.
    pub packages: Vec<String>,
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
    /// modules), keyed by content hash. [`persist_build`](super::persist_build)
    /// writes these to the store's `prelink/` area before the snapshot pointer
    /// flips, so a rooted manifest's prelink references always resolve. A cache
    /// *hit* re-uses the prior blob already in the store and contributes
    /// nothing here.
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
/// The check pre-pass ([`check_prepass`](super::check_prepass)) checks every
/// cache-missing module up front, so a single cold build can fail in several
/// modules at once; each is captured here and they surface together (see
/// [`BuildError::TypeCheck`]).
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
                // A terse structural summary only. The *rendered* diagnostics —
                // spanned, source-context, `ambient check`-identical — live in
                // the CLI's `report_build_error`; this `Display` exists for
                // logging/`Debug`-adjacent uses and must not fork a second,
                // lossier rendering of the errors themselves. Callers wanting
                // the details read the structured `failures` field.
                let modules = failures
                    .iter()
                    .map(|failure| failure.module.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "type checking failed in {} module(s): {modules}",
                    failures.len()
                )
            }
            Self::Compile { module, error } => write!(f, "compile error in {module}: {error}"),
            Self::ImportCycle { message } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for BuildError {}

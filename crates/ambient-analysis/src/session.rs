//! Incremental analysis session: a per-module check memo over a live package.
//!
//! `ambient check` and the language server both analyze the same package the
//! same way; the LSP just does it again on every keystroke. An
//! [`AnalysisSession`] makes that warm path cheap without changing *what* is
//! reported: a module's check-level result (parse errors, type errors, typed
//! AST) is memoized on the exact inputs it depends on, so a warm hit replays a
//! byte-identical result and only genuinely-affected modules re-check.
//!
//! # The memo key
//!
//! One key per module, folded by the engine's own
//! [`module_cache_key`](ambient_engine::build::module_cache_key) — reusing the
//! exact fold `build_package`'s Phase 3 cache uses, so the dependency /
//! dispatch channels can never drift from the compiler's:
//!
//! ```text
//! key(A) = blake3(source_hash(A) ‖ dispatch_surface_hash ‖
//!                 for each resolve-dep D sorted: id(D) ‖ interface_hash(D))
//! ```
//!
//! The **own-source** slot is a hash of A's *raw source bytes*, not the build
//! cache's span-free `resolved_ast_hash`. Diagnostics are span-sensitive and
//! include **parse errors** (which never enter any AST hash at all), so a
//! span-free key would let a body edit that only shifts spans — or a mid-edit
//! reformat of a broken region — serve stale diagnostics. The build cache is
//! immune (compiled objects are span-free by design); analysis is not, so it
//! keys the one channel that must be exact on the source itself. The
//! dependency channel stays span-free (interface hashes): a dep's span shift
//! doesn't change the consumer's diagnostics, so consumers must *not*
//! re-check for it.
//!
//! Unlike the build cache this also needs **no link validation**: checking
//! never consumes a callee's final content hash, so a module's diagnostics are
//! a pure function of these check-level inputs. (The natives-contract channel
//! is likewise dropped — `extern fn` signatures live in the AST; a native's
//! content identity is a runtime/link fact, not a type-check input.)
//!
//! `AMBIENT_ANALYSIS_VERIFY=1` mirrors the build cache's `AMBIENT_CACHE_VERIFY`:
//! every memo *hit* is recomputed cold and asserted byte-identical, so an
//! under-invalidation bug panics loudly rather than serving a stale
//! diagnostic. Off in ordinary runs (the hit path never recomputes).
//!
//! Invalidation is **by construction**: a dependent's key embeds its deps'
//! interface hashes, so an interface-changing edit moves every dependent's key
//! naturally, with no dependency-walking logic here. A body-only edit moves
//! only the edited module's own `resolved_ast_hash`, so exactly one module
//! re-checks. Dispatch/coherence edits move `dispatch_surface_hash`, which is
//! in every key.
//!
//! # Import cycles
//!
//! A cycle is a package-graph fact, not a per-module input: a dependent's edit
//! can create or dissolve a cycle without moving this module's key. So the
//! cycle set is **not** memoized — it is recomputed once per registry revision
//! (via [`cycles_by_member`](ambient_engine::module_cycles::cycles_by_member),
//! from dependency edges the session already holds — no per-module re-resolve,
//! fixing the Phase 0 O(modules²) cost) and overlaid at serve time. The
//! rendering is identical to `import_cycle_containing`, preserving parity.
//!
//! # Registry incrementality
//!
//! The session owns one persistent [`ModuleRegistry`]. A body-only edit
//! (interface hash unchanged) re-registers **only** the edited module — every
//! other module's resolved AST is provably unaffected, since resolution of a
//! foreign reference depends only on the target's *exports*, which are
//! unchanged. An interface-changing edit (or a new/removed module) forces a
//! full registry rebuild: a dependent's stored resolved AST could now resolve
//! differently, and serving stale resolution would be a correctness bug. The
//! memo still absorbs the cost — only the changed module and its dependents
//! re-check; unrelated modules hit — so the rebuild re-runs the cheap resolve
//! pass, never the checker, for the untouched majority.
//!
//! ## Reverse-dep-scoped re-resolve (deferred)
//!
//! The full rebuild on an interface change re-resolves *every* module even
//! though only the changed module's transitive reverse-deps can resolve
//! differently. A scoped alternative is expressible from state the session
//! already holds — invert [`deps`](AnalysisSession::deps) to a reverse-dep
//! graph, re-register the changed module resolved, then re-resolve only its
//! transitive reverse-deps (transitive because a direct importer that
//! `pub use`-re-exports a changed symbol shifts *its* exports too), leaving
//! unrelated modules' resolved ASTs, interfaces, and occurrences untouched;
//! dispatch and cycles recompute from the (mostly unchanged) interfaces.
//!
//! It is deferred deliberately, not for lack of a path:
//! - **The dominant cost is already gone.** The memo spares the *checker*
//!   (type inference — the expensive pass) for every non-dependent. A rebuild's
//!   residual cost is the *resolve* pass (name canonicalization, no inference)
//!   plus interface/occurrence folds — all O(modules) and cheap next to
//!   checking. Scoping them trims a small constant.
//! - **The correctness surface is large.** Serving one stale resolved AST is a
//!   silent miscompile (this module's whole reason for the full rebuild). A
//!   partial re-resolve must get the transitive reverse-dep frontier exactly
//!   right — including re-export chains and dispatch-surface interactions — and
//!   would need its own standing oracle to be trustworthy.
//!
//! The occurrence index *is* scoped on the body-only path (its identities are
//! span-free `Fqn`s, so the reverse-dep frontier there is empty by
//! construction); extending that scoping across an interface change is the
//! natural follow-up, gated on the reverse-dep re-resolve above.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use ambient_engine::ast::Span;
use ambient_engine::build::{dep_interface_hashes, module_cache_key, per_module_dispatch_hashes};
use ambient_engine::module_cycles::cycles_by_member;
use ambient_engine::module_interface::{
    ModuleInterface, ModuleInterfaceSummary, build_interfaces, module_ast_hash, module_source_path,
    structured_items,
};
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;

use crate::occurrences::{Occurrence, collect_occurrences};
use crate::package::{AnalysisPackage, ModuleDeps};
use crate::{AnalysisResult, Diagnostic, check_without_cycle};

/// A memoized module: the check-level result and the key it was computed at.
struct MemoEntry {
    key: [u8; 32],
    value: Arc<AnalysisResult>,
}

/// A package opened for incremental analysis: the persistent registry, the
/// per-module derived state (interfaces, dependency edges, cycles), and the
/// per-module check memo. Owns its [`AnalysisPackage`]; edit modules through
/// [`edit_module`](Self::edit_module) so the derived state stays coherent.
pub struct AnalysisSession {
    package: AnalysisPackage,
    /// The shared registry every open document is checked against. A fresh
    /// `Arc` per revision; open-document analyses point at whichever revision
    /// they were last checked under (their keys capture the difference).
    registry: Arc<ModuleRegistry>,
    /// Per-module interface summary (interface hash + resolved-AST hash),
    /// keyed by canonical module identity — the build-global view the keys
    /// fold over.
    interfaces: BTreeMap<String, ModuleInterfaceSummary>,
    /// Each package module's **narrowed** dispatch-key input, keyed by canonical
    /// identity — the per-module analogue of the old build-global dispatch hash.
    /// A module's key folds only the impl shapes it can dispatch (plus the global
    /// unconditional/colliding/ability bytes), so an impl add on an unrelated
    /// package type no longer re-checks it. See `build::dispatch_scope`.
    ///
    /// Recomputed only by [`rebuild`](Self::rebuild); the incremental
    /// unchanged-interface edit path leaves it untouched, which is sound because
    /// that path guarantees no impl/interface change anywhere (an impl add or
    /// signature/body edit all move the edited module's interface hash → a full
    /// rebuild), so no module's dispatch relevance can change beneath a stale
    /// entry.
    module_dispatch: BTreeMap<String, [u8; 32]>,
    /// Each package module's resolve-pass dependency edges, keyed by canonical
    /// identity. The cache keys fold dep interface hashes; the cycle graph
    /// reads the dotted-path + scope form.
    deps: BTreeMap<String, ModuleDeps>,
    /// The package's import cycles this revision, keyed by dotted module path
    /// (`ModuleId::module_path_string`) — the same key
    /// `import_cycle_containing` matches on.
    cycles: BTreeMap<String, ambient_engine::module_cycles::ImportCycle>,
    /// Per-module check memo, keyed by canonical identity.
    memo: HashMap<String, MemoEntry>,
    /// The occurrence index backing find-references and rename: every
    /// definition and reference site of every symbol, keyed by dotted module
    /// path (`ModulePath::to_string` — the key [`AnalysisPackage::modules`]
    /// uses, so the LSP renderer maps a module back to its file with no
    /// re-derivation). Owned here, not in the LSP: it is an analysis fact the
    /// server only renders.
    ///
    /// Rebuilt module-scoped: `Item` occurrence identities are span-free
    /// [`Fqn`](ambient_engine::fqn::Fqn)s, so a body edit that shifts spans in
    /// one module leaves every *other* module's entries valid. A body-only edit
    /// re-collects only the edited module; an interface change or a module-set
    /// change rebuilds all (through [`rebuild`](Self::rebuild)).
    occurrences: BTreeMap<String, Vec<Occurrence>>,
    /// The package's last-build snapshot manifest, if one was loaded via
    /// [`load_snapshot`](Self::load_snapshot). Its structured item index backs
    /// workspace-symbol search for modules the live session does not cover
    /// (live always wins — see [`crate::symbols`]). `None` when no build has
    /// run, the store is absent, or the snapshot is missing/corrupt: the
    /// session works identically without it.
    snapshot: Option<ambient_engine::disk_store::BuildManifest>,
    /// How many module checks actually ran (memo misses). Cumulative
    /// instrumentation for the incremental tests; never affects behavior.
    rechecks: usize,
    /// How many per-module occurrence lists were re-collected. Cumulative
    /// instrumentation proving a body-only edit re-walks exactly one module's
    /// occurrences; never affects behavior.
    occurrence_rebuilds: usize,
    /// The `AMBIENT_ANALYSIS_VERIFY=1` oracle: on every memo *hit*, recompute
    /// the module cold and assert byte-identical diagnostics — the analysis
    /// mirror of the build cache's `AMBIENT_CACHE_VERIFY`. Resolved once at
    /// construction (env is process-global); off in ordinary runs.
    verify: bool,
}

impl AnalysisSession {
    /// Open a session over `package`: build the registry, derive interfaces /
    /// dispatch / dependency edges / cycles. The memo starts empty, so the
    /// first [`analyze_all`](Self::analyze_all) is a full cold pass.
    #[must_use]
    pub fn new(package: AnalysisPackage) -> Self {
        let mut session = Self {
            package,
            registry: Arc::new(ModuleRegistry::new()),
            interfaces: BTreeMap::new(),
            module_dispatch: BTreeMap::new(),
            deps: BTreeMap::new(),
            cycles: BTreeMap::new(),
            snapshot: None,
            memo: HashMap::new(),
            occurrences: BTreeMap::new(),
            rechecks: 0,
            occurrence_rebuilds: 0,
            verify: std::env::var("AMBIENT_ANALYSIS_VERIFY")
                .is_ok_and(|v| v.eq_ignore_ascii_case("1")),
        };
        session.rebuild();
        session
    }

    /// The underlying package (its parsed modules, paths, manifest info).
    #[must_use]
    pub fn package(&self) -> &AnalysisPackage {
        &self.package
    }

    /// The current shared registry (an `Arc` clone is cheap — module ASTs are
    /// `Arc`-shared). Open-document handlers resolve names through this.
    #[must_use]
    pub fn registry(&self) -> &Arc<ModuleRegistry> {
        &self.registry
    }

    /// Total module checks run so far (memo misses). Instrumentation.
    #[must_use]
    pub fn rechecks(&self) -> usize {
        self.rechecks
    }

    /// Total per-module occurrence re-collections so far. Instrumentation:
    /// a full rebuild bumps this once per module, a body-only edit once.
    #[must_use]
    pub fn occurrence_rebuilds(&self) -> usize {
        self.occurrence_rebuilds
    }

    /// The occurrence list for one module, or `None` if the module is not in
    /// the package. The LSP renders these into reference/rename locations; it
    /// never collects occurrences itself.
    #[must_use]
    pub fn occurrences_for(&self, path: &ModulePath) -> Option<&[Occurrence]> {
        self.occurrences.get(&path.to_string()).map(Vec::as_slice)
    }

    /// Load the package's last-build snapshot index (read-only) so
    /// workspace-symbol search can cover any module the live session does not.
    ///
    /// Reads `<root>/.ambient/store` only if it already exists — never creates
    /// it, so an in-memory package (the REPL, with a notional root) is
    /// untouched. A missing store, absent pointer, or corrupt manifest all
    /// leave the index empty; the session is fully functional without it.
    pub fn load_snapshot(&mut self) {
        self.snapshot = read_package_snapshot(&self.package.root);
    }

    /// Every workspace symbol matching `query`, from the live interfaces and
    /// (for modules the live set lacks) the loaded snapshot. Live analysis
    /// state always wins per module — see [`crate::symbols`].
    #[must_use]
    pub fn workspace_symbols(&self, query: &str) -> Vec<crate::symbols::WorkspaceSymbol> {
        crate::symbols::workspace_symbols(query, &self.interfaces, self.snapshot.as_ref())
    }

    /// Re-parse and re-integrate one edited module, updating the registry and
    /// derived state incrementally where sound and rebuilding fully where an
    /// interface change could otherwise leave a dependent resolving stale.
    ///
    /// The memo is never cleared: after the update, [`analyze_all`] /
    /// [`analyze_module`] serve unchanged modules from cache by key match.
    pub fn edit_module(&mut self, path: &ModulePath, source: String) {
        self.package.insert_module(path.clone(), source);
        let id_str = self.registry.module_id(path).to_string();

        // A never-before-seen module (new file, or a module added after
        // discovery) changes the module set: rebuild so every module's view is
        // consistent and interfaces/cycles account for it.
        if !self.deps.contains_key(&id_str) {
            self.rebuild();
            return;
        }

        // Incremental single-module re-registration. Resolution of *other*
        // modules' references into this one depends only on this module's
        // exports; if those are unchanged (interface hash equal), their stored
        // resolved ASTs remain correct and need no touch.
        let raw_ast = Arc::new(self.package.modules[&path.to_string()].ast.clone());
        let new_deps = {
            let reg = Arc::make_mut(&mut self.registry);
            reg.register(path, Arc::clone(&raw_ast));
            let mut resolved = (*raw_ast).clone();
            let outcome = ambient_engine::resolve::resolve_module(&mut resolved, path, reg);
            reg.register(path, Arc::new(resolved));
            outcome.deps
        };

        let Some(new_summary) = self.summarize(path) else {
            // The module vanished from the registry (should not happen right
            // after registering it) — fail safe to a full rebuild.
            self.rebuild();
            return;
        };
        let unchanged_interface = self
            .interfaces
            .get(&id_str)
            .is_some_and(|old| old.interface_hash == new_summary.interface_hash);

        if unchanged_interface {
            // Body-only (or import-only) edit: only this module's own
            // `resolved_ast_hash` moved, so only its key changes. Dispatch and
            // every other interface are byte-identical, so their keys hold.
            let module_id = new_summary.module.clone();
            self.interfaces.insert(id_str.clone(), new_summary);
            self.deps.insert(
                id_str,
                ModuleDeps {
                    module_id,
                    deps: new_deps,
                },
            );
            // Import edges can change without the interface changing (adding a
            // `use` + call in a body), so the cycle graph must be recomputed.
            self.recompute_cycles();
            // Only this module's spans (and same-module references) moved; every
            // other module's occurrences target the unchanged `Fqn`s, so they
            // stay valid. Re-collect just the edited module.
            self.rebuild_occurrences_for(path);
            if self.verify {
                self.assert_occurrences_scoped_matches_full();
            }
        } else {
            // The exported surface moved: a dependent's resolution — and thus
            // the registry state the checker reads for it — may now differ.
            // Rebuild to keep every resolved AST current; the memo still spares
            // the checker for everything but the changed module + dependents.
            //
            // A reverse-dep-scoped re-resolve (only re-resolving the transitive
            // reverse-deps of this module, leaving unrelated modules' resolved
            // ASTs and occurrences untouched) is possible from `self.deps`, but
            // deferred — see the module-level "Registry incrementality" docs for
            // why the win is marginal and the correctness risk is not.
            self.rebuild();
        }
    }

    /// Analyze every package module, memoized. Keyed by dotted module path
    /// (matching [`AnalysisPackage::analyze_all`]). Used by `ambient check`
    /// (a single cold pass) and by warm reanalysis.
    #[must_use]
    pub fn analyze_all(&mut self) -> HashMap<String, AnalysisResult> {
        // Detach the module list from the `&self.package` borrow so serving
        // (which mutates the memo) is free of aliasing. Cloning the sources is
        // cheap next to the checks the memo saves.
        let modules: Vec<(String, ModulePath, String)> = self
            .package
            .modules
            .values()
            .map(|m| (m.path.to_string(), m.path.clone(), m.source.clone()))
            .collect();
        modules
            .into_iter()
            .map(|(key, path, source)| (key, self.serve(&path, &source)))
            .collect()
    }

    /// Analyze one package module, memoized. `None` if the module isn't in the
    /// package. Keyed lookups mirror [`analyze_all`](Self::analyze_all).
    #[must_use]
    pub fn analyze_module(&mut self, path: &ModulePath) -> Option<AnalysisResult> {
        let source = self.package.modules.get(&path.to_string())?.source.clone();
        Some(self.serve(path, &source))
    }

    // ── internals ──────────────────────────────────────────────────────────

    /// Serve a module's [`AnalysisResult`]: a memo hit replays the cached
    /// check-level result; a miss re-checks and stores it. Either way the
    /// per-revision import-cycle overlay is applied fresh.
    fn serve(&mut self, path: &ModulePath, source: &str) -> AnalysisResult {
        let id_str = self.registry.module_id(path).to_string();
        let key = self.cache_key(&id_str, *blake3::hash(source.as_bytes()).as_bytes());

        let cached = match (&key, self.memo.get(&id_str)) {
            (Some(k), Some(entry)) if entry.key == *k => {
                let hit = Arc::clone(&entry.value);
                if self.verify {
                    // Oracle: a memo hit must be byte-identical to a fresh cold
                    // check of the same source. A mismatch is an
                    // under-invalidation bug — fail loudly, never serve stale.
                    let fresh = check_without_cycle(source, Some(path), Some(&self.registry), None);
                    assert_diagnostics_equivalent(&id_str, &hit, &fresh);
                }
                hit
            }
            _ => {
                let result = check_without_cycle(source, Some(path), Some(&self.registry), None);
                self.rechecks += 1;
                let value = Arc::new(result);
                if let Some(k) = key {
                    self.memo.insert(
                        id_str.clone(),
                        MemoEntry {
                            key: k,
                            value: Arc::clone(&value),
                        },
                    );
                }
                value
            }
        };

        let mut result = (*cached).clone();
        result.import_cycle = self.cycle_for(path);
        result
    }

    /// This module's cache key from its own source hash, or `None` if a
    /// dependency lacks an interface summary (then it never hits and always
    /// re-checks — fail safe).
    fn cache_key(&self, id_str: &str, source_hash: [u8; 32]) -> Option<[u8; 32]> {
        // Reuse the engine's exact dep fold (sort + dedup + missing-dep guard)
        // via a one-entry adaptor, so the dep channel matches `build_package`'s.
        let deps = self.deps.get(id_str)?;
        let mut one = BTreeMap::new();
        one.insert(
            id_str.to_string(),
            deps.deps.iter().map(ToString::to_string).collect(),
        );
        let dep_hashes = dep_interface_hashes(&one, id_str, &self.interfaces)?;
        // The own-source slot is the raw source hash (spans + parse errors
        // matter for diagnostics); natives contract is a non-input, pass zero.
        // See the module docs for why analysis deviates from the build key here.
        // The narrowed per-module dispatch input (a missing entry — never in
        // practice — falls back to zero, so the module simply always re-checks).
        let dispatch = self
            .module_dispatch
            .get(id_str)
            .copied()
            .unwrap_or([0u8; 32]);
        Some(module_cache_key(
            source_hash,
            [0u8; 32],
            dispatch,
            &dep_hashes,
        ))
    }

    /// The import-cycle diagnostic for a module this revision, if any —
    /// byte-identical to `analyze_with_registry`'s per-module rendering.
    fn cycle_for(&self, path: &ModulePath) -> Option<Diagnostic> {
        let key = self.registry.module_id(path).module_path_string();
        self.cycles
            .get(&key)
            .map(|cycle| Diagnostic::error(Span::new(0, 0), cycle.describe(), None))
    }

    /// Full rebuild: fresh registry + all derived state. The memo is retained
    /// (keys decide reuse), so only changed/dependent modules re-check.
    fn rebuild(&mut self) {
        let (registry, deps) = self.package.build_registry_with_deps();
        self.registry = Arc::new(registry);
        self.deps = deps;
        self.interfaces = build_interfaces(&self.registry);
        // The per-module narrowed dispatch inputs. Built from the same
        // `(module id -> dep module ids)` graph the cache keys fold, so the
        // editor and the compiler derive identical narrowing.
        let dep_ids: BTreeMap<String, Vec<String>> = self
            .deps
            .iter()
            .map(|(id, d)| (id.clone(), d.deps.iter().map(ToString::to_string).collect()))
            .collect();
        self.module_dispatch = per_module_dispatch_hashes(&self.registry, &dep_ids);
        self.recompute_cycles();
        self.rebuild_all_occurrences();
    }

    /// Re-collect every module's occurrence list against the current registry,
    /// dropping any entry whose module is gone. Used by the full [`rebuild`].
    fn rebuild_all_occurrences(&mut self) {
        self.occurrences.clear();
        let paths: Vec<ModulePath> = self
            .package
            .modules
            .values()
            .map(|m| m.path.clone())
            .collect();
        for path in paths {
            self.rebuild_occurrences_for(&path);
        }
    }

    /// Re-collect one module's occurrence list against the current registry.
    /// Every other module's list is left untouched — sound because `Item`
    /// occurrence identities are span-free `Fqn`s (see [`crate::occurrences`]).
    fn rebuild_occurrences_for(&mut self, path: &ModulePath) {
        let key = path.to_string();
        // Borrow package + registry immutably to collect, release, then insert
        // into the disjoint `occurrences` field. `collect_occurrences` returns
        // an owned Vec, so no AST clone is needed.
        let collected = self
            .package
            .modules
            .get(&key)
            .map(|m| collect_occurrences(&m.ast, &m.path, &self.registry));
        match collected {
            Some(occ) => {
                self.occurrences.insert(key, occ);
                self.occurrence_rebuilds += 1;
            }
            None => {
                self.occurrences.remove(&key);
            }
        }
    }

    /// The interface summary of one module, read from the current registry —
    /// matching what [`build_interfaces`] produces for it. `None` only if the
    /// module isn't registered (a caller-side invariant violation).
    fn summarize(&self, path: &ModulePath) -> Option<ModuleInterfaceSummary> {
        let info = self.registry.get(path)?;
        let resolved_ast_hash = module_ast_hash(&info.module);
        let interface = ModuleInterface::from_module(&self.registry, path);
        let module = self.registry.module_id(path);
        let source_path = module_source_path(&module, info);
        let items = structured_items(&info.module);
        Some(ModuleInterfaceSummary {
            module,
            interface_hash: interface.interface_hash(),
            interface,
            resolved_ast_hash,
            source_path,
            items,
        })
    }

    /// Recompute the package's import-cycle set from the current dependency
    /// edges — one graph pass, no re-resolve.
    fn recompute_cycles(&mut self) {
        self.cycles = cycles_for(&self.deps);
    }

    /// The `AMBIENT_ANALYSIS_VERIFY` oracle for the occurrence index: after a
    /// scoped (single-module) rebuild, the incrementally-maintained index must
    /// be byte-for-byte what a full cold re-collection of every module would
    /// produce. A divergence means a body edit stranded some module's
    /// references — the exact bug span-free `Fqn` keying is meant to prevent.
    /// Panics loudly on mismatch; never runs off the verify path.
    fn assert_occurrences_scoped_matches_full(&self) {
        let full: BTreeMap<String, Vec<(u32, u32, String, bool)>> = self
            .package
            .modules
            .values()
            .map(|m| {
                let occ = collect_occurrences(&m.ast, &m.path, &self.registry);
                (m.path.to_string(), normalize_occurrences(&occ))
            })
            .collect();
        let scoped: BTreeMap<String, Vec<(u32, u32, String, bool)>> = self
            .occurrences
            .iter()
            .map(|(k, occ)| (k.clone(), normalize_occurrences(occ)))
            .collect();
        assert!(
            scoped == full,
            "AMBIENT_ANALYSIS_VERIFY: scoped occurrence index diverged from a \
             full rebuild:\n  scoped {scoped:?}\n  full   {full:?}"
        );
    }
}

/// A stable, order-independent projection of a module's occurrences for the
/// verify oracle: every site's span, identity, and definition flag. The
/// identity string is the `Fqn` for an item and a module-scoped binding tag for
/// a local — distinct across classes, so two normalized lists are equal iff the
/// underlying indexes are.
fn normalize_occurrences(occ: &[Occurrence]) -> Vec<(u32, u32, String, bool)> {
    use crate::occurrences::SymbolTarget;
    let mut out: Vec<(u32, u32, String, bool)> = occ
        .iter()
        .map(|o| {
            let identity = match &o.target {
                SymbolTarget::Item { fqn, .. } => format!("item:{fqn}"),
                SymbolTarget::Local {
                    module, binding_id, ..
                } => format!("local:{module}:{binding_id:?}"),
            };
            (o.span.start, o.span.end, identity, o.is_definition)
        })
        .collect();
    out.sort();
    out
}

/// Read a package's current build snapshot, read-only and best-effort.
///
/// Returns `None` — never an error, never a side effect — when the store
/// directory does not yet exist (no build has run), when there is no snapshot
/// pointer, or when the pointed-at manifest is missing/corrupt/unknown-version
/// ([`DiskStore::current_snapshot`] already collapses those to `Ok(None)`).
/// The existence guard matters: opening a store *creates* its directory, so an
/// in-memory package (the REPL) must not open one it never built.
fn read_package_snapshot(
    root: &std::path::Path,
) -> Option<ambient_engine::disk_store::BuildManifest> {
    use ambient_engine::disk_store::DiskStore;
    let store_path = DiskStore::package_store_path(root);
    if !store_path.exists() {
        return None;
    }
    DiskStore::open_package(root)
        .ok()?
        .current_snapshot()
        .ok()?
}

/// The package's import cycles keyed by dotted module path
/// (`ModuleId::module_path_string`), from resolve-pass dependency edges.
///
/// One graph pass, no re-resolve — shared by the incremental session and the
/// one-shot [`AnalysisPackage::analyze_all`](crate::package::AnalysisPackage::analyze_all)
/// so both drop the per-module O(modules²) `import_cycle_containing` loop and
/// render cycles identically. Each edge is restricted to its source module's
/// own [`Scope`](ambient_engine::fqn::Scope), so core/platform can never enter
/// a package cycle (they are separately guaranteed acyclic and cannot import
/// user code).
#[must_use]
pub(crate) fn cycles_for(
    deps: &BTreeMap<String, ModuleDeps>,
) -> BTreeMap<String, ambient_engine::module_cycles::ImportCycle> {
    let graph: BTreeMap<String, Vec<String>> = deps
        .values()
        .map(|md| {
            let scope = &md.module_id.scope;
            let edges = md
                .deps
                .iter()
                .filter(|d| &d.scope == scope)
                .map(ambient_engine::fqn::ModuleId::module_path_string)
                .collect();
            (md.module_id.module_path_string(), edges)
        })
        .collect();
    cycles_by_member(&graph)
}

/// The `AMBIENT_ANALYSIS_VERIFY` oracle: assert a memo-served result carries
/// the same (pre-cycle-overlay) diagnostics a fresh cold check produces.
/// Panics with a precise diff on mismatch — a standing under-invalidation
/// detector, never a recoverable condition.
fn assert_diagnostics_equivalent(module_id: &str, hit: &AnalysisResult, fresh: &AnalysisResult) {
    let hit_diags = hit.diagnostics();
    let fresh_diags = fresh.diagnostics();
    assert!(
        hit_diags == fresh_diags,
        "AMBIENT_ANALYSIS_VERIFY: stale memo hit for `{module_id}`: \
         cached {hit_diags:?} vs fresh {fresh_diags:?}"
    );
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;

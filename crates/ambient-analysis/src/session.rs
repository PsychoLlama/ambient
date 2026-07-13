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

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use ambient_engine::ast::Span;
use ambient_engine::build::{dep_interface_hashes, module_cache_key};
use ambient_engine::module_cycles::cycles_by_member;
use ambient_engine::module_interface::{
    ModuleInterface, ModuleInterfaceSummary, build_interfaces, dispatch_surface_hash,
    module_ast_hash,
};
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;

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
    /// The build-global dispatch/coherence surface hash.
    dispatch: [u8; 32],
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
    /// How many module checks actually ran (memo misses). Cumulative
    /// instrumentation for the incremental tests; never affects behavior.
    rechecks: usize,
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
            dispatch: [0u8; 32],
            deps: BTreeMap::new(),
            cycles: BTreeMap::new(),
            memo: HashMap::new(),
            rechecks: 0,
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
        } else {
            // The exported surface moved: a dependent's resolution — and thus
            // the registry state the checker reads for it — may now differ.
            // Rebuild to keep every resolved AST current; the memo still spares
            // the checker for everything but the changed module + dependents.
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
            (Some(k), Some(entry)) if entry.key == *k => Arc::clone(&entry.value),
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
        Some(module_cache_key(
            source_hash,
            [0u8; 32],
            self.dispatch,
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
        self.dispatch = *dispatch_surface_hash(&self.interfaces).as_bytes();
        self.recompute_cycles();
    }

    /// The interface summary of one module, read from the current registry —
    /// matching what [`build_interfaces`] produces for it. `None` only if the
    /// module isn't registered (a caller-side invariant violation).
    fn summarize(&self, path: &ModulePath) -> Option<ModuleInterfaceSummary> {
        let resolved_ast_hash = module_ast_hash(&self.registry.get(path)?.module);
        let interface = ModuleInterface::from_module(&self.registry, path);
        Some(ModuleInterfaceSummary {
            module: self.registry.module_id(path),
            interface_hash: interface.interface_hash(),
            interface,
            resolved_ast_hash,
        })
    }

    /// Recompute the package's import-cycle set from the current dependency
    /// edges — one graph pass, no re-resolve.
    fn recompute_cycles(&mut self) {
        self.cycles = cycles_for(&self.deps);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_package(files: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new().expect("temp dir");
        let root = dir.path();
        fs::write(
            root.join("ambient.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n\n[build]\nsrc = \"src\"\n",
        )
        .expect("manifest");
        let src = root.join("src");
        fs::create_dir_all(&src).expect("src");
        for (name, content) in files {
            fs::write(src.join(name), content).expect("module");
        }
        dir
    }

    fn open(dir: &TempDir) -> AnalysisSession {
        let package = AnalysisPackage::open(dir.path()).expect("open package");
        AnalysisSession::new(package)
    }

    fn module_path(name: &str) -> ModulePath {
        ModulePath::from_relative_file_path(std::path::Path::new(&format!("{name}.ab")))
            .expect("module path")
    }

    /// A stable, comparable view of a module's diagnostics.
    fn diags(result: &AnalysisResult) -> Vec<(u32, u32, String)> {
        result
            .diagnostics()
            .into_iter()
            .map(|d| (d.span.start, d.span.end, d.message))
            .collect()
    }

    /// The warm result must equal a fresh cold analysis of the session's
    /// *current* (in-memory, possibly edited) sources — the correctness bar for
    /// the whole phase. `session.package().analyze_all()` is the cold path over
    /// exactly those sources, sharing none of the session's memo state.
    fn assert_matches_cold(session: &AnalysisSession, warm: &HashMap<String, AnalysisResult>) {
        let cold = session.package().analyze_all();
        let mut keys: Vec<_> = cold.keys().cloned().collect();
        keys.sort();
        for key in keys {
            assert_eq!(
                diags(&warm[&key]),
                diags(&cold[&key]),
                "warm vs cold diagnostics differ for `{key}`"
            );
        }
    }

    #[test]
    fn cold_pass_checks_every_module_once() {
        let dir = write_package(&[
            (
                "main.ab",
                "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
            ),
            ("utils.ab", "pub fn helper(): Number { 1 }\n"),
        ]);
        let mut session = open(&dir);
        let results = session.analyze_all();
        assert_eq!(session.rechecks(), 2, "cold: both modules checked");
        for (key, r) in &results {
            assert!(r.diagnostics().is_empty(), "{key}: {:?}", diags(r));
        }
    }

    #[test]
    fn body_only_edit_rechecks_exactly_one_module() {
        let dir = write_package(&[
            (
                "main.ab",
                "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
            ),
            ("utils.ab", "pub fn helper(): Number { 1 }\n"),
        ]);
        let mut session = open(&dir);
        let _ = session.analyze_all();
        let base = session.rechecks();

        // Edit `main`'s body only — its signature (interface) is unchanged.
        session.edit_module(
            &module_path("main"),
            "use pkg::utils::helper;\npub fn run(): Number { helper() + 0 }\n".to_string(),
        );
        let warm = session.analyze_all();
        assert_eq!(
            session.rechecks() - base,
            1,
            "only the edited module re-checks"
        );
        assert_matches_cold(&session, &warm);
    }

    #[test]
    fn interface_edit_rechecks_changed_module_and_dependents() {
        let dir = write_package(&[
            (
                "main.ab",
                "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
            ),
            ("utils.ab", "pub fn helper(): Number { 1 }\n"),
            ("other.ab", "pub fn unrelated(): Number { 2 }\n"),
        ]);
        let mut session = open(&dir);
        let _ = session.analyze_all();
        let base = session.rechecks();

        // Change `helper`'s signature: `main` depends on it and must re-check;
        // `other` does not and must stay memoized. (Not an impl/ability edit,
        // so the dispatch surface is untouched and `other`'s key holds.)
        session.edit_module(
            &module_path("utils"),
            "pub fn helper(): String { \"x\" }\n".to_string(),
        );
        let warm = session.analyze_all();
        assert_eq!(
            session.rechecks() - base,
            2,
            "changed module + its one dependent re-check, `other` stays memoized"
        );
        assert_matches_cold(&session, &warm);
    }

    #[test]
    fn dispatch_surface_edit_flows_through_every_key() {
        // An impl edit moves the changed module's interface (impls are in it)
        // and the build-global dispatch surface, so it is an interface-class
        // change; the result must still match a cold analysis.
        let dir = write_package(&[
            (
                "shapes.ab",
                "unique(A1B2C3D4-0000-0000-0000-000000000001) struct P { x: Number }\n\
                 impl P {\n    fn get(self): Number { self.x }\n}\n",
            ),
            (
                "main.ab",
                "use pkg::shapes::P;\npub fn run(): Number { P { x: 1 }.get() }\n",
            ),
        ]);
        let mut session = open(&dir);
        let cold = session.analyze_all();
        assert_matches_cold(&session, &cold);

        session.edit_module(
            &module_path("shapes"),
            "unique(A1B2C3D4-0000-0000-0000-000000000001) struct P { x: Number }\n\
             impl P {\n    fn get(self): Number { self.x + 0 }\n}\n"
                .to_string(),
        );
        let warm = session.analyze_all();
        assert_matches_cold(&session, &warm);
    }

    #[test]
    fn unrelated_impl_body_edit_leaves_unrelated_modules_cached() {
        // Phase 5 step 2: the dispatch surface is body-free. An impl *body*
        // edit in `shapes` re-checks `shapes` (its own source) and its
        // dependent `consumer` (the full interface hash still retains bodies,
        // a spurious-but-safe re-check through the dependency channel), but
        // leaves `unrelated` — which names neither the type nor the impl's
        // module — memoized. Under the old body-bearing *global* dispatch
        // hash the edit moved every module's key, so all three re-checked.
        let dir = write_package(&[
            (
                "shapes.ab",
                "pub unique(A1B2C3D4-0000-0000-0000-000000000042) struct P { x: Number }\n\
                 impl P {\n    fn get(self): Number { self.x }\n}\n",
            ),
            (
                "consumer.ab",
                "use pkg::shapes::P;\npub fn run(): Number { P { x: 1 }.get() }\n",
            ),
            ("unrelated.ab", "pub fn f(): Number { 2 }\n"),
        ]);
        let mut session = open(&dir);
        let _ = session.analyze_all();
        let base = session.rechecks();

        session.edit_module(
            &module_path("shapes"),
            "pub unique(A1B2C3D4-0000-0000-0000-000000000042) struct P { x: Number }\n\
             impl P {\n    fn get(self): Number { self.x + 0 }\n}\n"
                .to_string(),
        );
        let warm = session.analyze_all();
        assert_eq!(
            session.rechecks() - base,
            2,
            "`shapes` (source) + `consumer` (dep) re-check; `unrelated` stays memoized"
        );
        assert_matches_cold(&session, &warm);
    }

    #[test]
    fn import_cycle_appears_and_clears_across_edits() {
        // Start acyclic, introduce a cycle via a body edit (no interface
        // change), then remove it. Cycles are recomputed per revision, so both
        // transitions match a cold analysis.
        let dir = write_package(&[
            ("a.ab", "pub fn ay(): Number { 1 }\n"),
            ("b.ab", "use pkg::a::ay;\npub fn bee(): Number { ay() }\n"),
        ]);
        let mut session = open(&dir);
        let clean = session.analyze_all();
        assert!(clean["a"].import_cycle.is_none());
        assert_matches_cold(&session, &clean);

        // `a` now imports `b`: a -> b -> a. `ay`'s signature is unchanged.
        session.edit_module(
            &module_path("a"),
            "use pkg::b::bee;\npub fn ay(): Number { bee() }\n".to_string(),
        );
        let cyclic = session.analyze_all();
        assert!(cyclic["a"].import_cycle.is_some(), "cycle must surface");
        assert!(cyclic["b"].import_cycle.is_some());
        assert_matches_cold(&session, &cyclic);

        // Break the cycle again.
        session.edit_module(&module_path("a"), "pub fn ay(): Number { 1 }\n".to_string());
        let healed = session.analyze_all();
        assert!(healed["a"].import_cycle.is_none(), "cycle must clear");
        assert_matches_cold(&session, &healed);
    }

    #[test]
    fn broken_module_does_not_poison_siblings_and_recovers() {
        let dir = write_package(&[
            (
                "main.ab",
                "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
            ),
            ("utils.ab", "pub fn helper(): Number { 1 }\n"),
        ]);
        let mut session = open(&dir);
        let _ = session.analyze_all();

        // Break `main` mid-edit (unparseable). `utils` must stay clean.
        session.edit_module(
            &module_path("main"),
            "pub fn run(: Number { helper(\n".to_string(),
        );
        let broken = session.analyze_all();
        assert!(
            !broken["main"].diagnostics().is_empty(),
            "broken main reports"
        );
        assert!(
            broken["utils"].diagnostics().is_empty(),
            "sibling not poisoned: {:?}",
            diags(&broken["utils"])
        );
        assert_matches_cold(&session, &broken);

        // Fix it: the whole package is clean again, matching cold.
        session.edit_module(
            &module_path("main"),
            "use pkg::utils::helper;\npub fn run(): Number { helper() }\n".to_string(),
        );
        let healed = session.analyze_all();
        for (key, r) in &healed {
            assert!(r.diagnostics().is_empty(), "{key}: {:?}", diags(r));
        }
        assert_matches_cold(&session, &healed);
    }

    #[test]
    fn span_shifting_edit_is_never_served_stale() {
        // Regression: the memo key hashes the raw source, not the span-free
        // structural AST hash. A leading-newline edit shifts a type error's
        // span without changing the module's structure; a span-free key would
        // replay the old span. The warm result must track the shift exactly.
        let dir = write_package(&[("main.ab", "pub fn run(): String { 42 }\n")]);
        let mut session = open(&dir);
        let before = session.analyze_all();
        let before_span = before["main"].diagnostics()[0].span.start;

        session.edit_module(
            &module_path("main"),
            "\n\npub fn run(): String { 42 }\n".to_string(),
        );
        let after = session.analyze_all();
        let after_span = after["main"].diagnostics()[0].span.start;

        assert_eq!(
            after_span,
            before_span + 2,
            "the type error span must shift"
        );
        assert_matches_cold(&session, &after);
    }

    #[test]
    fn analyze_module_and_analyze_all_agree() {
        let dir = write_package(&[
            (
                "main.ab",
                "use pkg::utils::helper;\npub fn run(): Number { helper() }\n",
            ),
            ("utils.ab", "pub fn helper(): Number { 1 }\n"),
        ]);
        let mut session = open(&dir);
        let all = session.analyze_all();
        let one = session.analyze_module(&module_path("main")).expect("main");
        assert_eq!(diags(&one), diags(&all["main"]));
    }
}

//! Incremental-compilation cache: the read side of Phase 3.
//!
//! A previous build's [`BuildManifest`] (loaded from the store's root pointer)
//! lets this build *skip check + compile* on modules whose inputs are
//! unchanged, materializing their objects straight from the disk store
//! instead. Correctness is absolute: a stale hit is a catastrophic bug, a
//! spurious miss is merely slow, so every decision here fails safe to "miss".
//!
//! # The invalidation model
//!
//! A package module's **cache key** folds the check-level inputs:
//!
//! ```text
//! key(A) = blake3(version ‖ resolved_ast_hash(A) ‖ natives_contract_hash ‖
//!                 dispatch_surface_hash ‖
//!                 for each dep D sorted: id(D) ‖ interface_hash(D))
//! ```
//!
//! A key match is **necessary but not sufficient**: compiled objects hard-link
//! the *final content hashes* of their callees, and those can move without any
//! key input changing (a private helper deep in the call graph whose body edit
//! re-hashes a trait-impl method a consumer dispatches, etc.). So a hit *also*
//! requires **link validation** against the current build's accumulated
//! linking state ([`LinkState`]): every cross-module callee the cached module
//! consumed must still resolve to the *same* hash, and every external code
//! reference in the cached objects must be either module-local or one of those
//! recorded bindings. Any mismatch, any uncovered reference, any object that
//! won't load ⇒ miss.
//!
//! # Consumed-link capture (intersection at write time)
//!
//! The consumed set is captured by **intersecting** the module's `imported`
//! linking table with the set of external hashes its objects actually
//! reference ([`external_code_refs`]). This is a conservative superset of the
//! true consumed set (an imported name whose hash coincides with a referenced
//! hash is recorded even if unused — a spurious miss at worst) and covers all
//! three cross-module channels uniformly, because every one lands in the
//! linking table keyed by a `NameKey` and is emitted into an object as an
//! external ref:
//!
//! - ordinary function/const calls → [`ObjectConstant::Ref`] / a dependency
//!   edge, keyed by the callee's `Fqn` ([`NameKey::Item`]);
//! - impl/inherent **dispatch symbols** (`<uuid>::<trait>::<method>`) → an
//!   ordinary `Ref`, keyed `NameKey::Bare`;
//! - **ability-method** default implementations → the `impl_fn` of an
//!   [`ObjectConstant::AbilityMethod`], keyed by the `<ability-uuid>::<method>`
//!   dispatch symbol (`NameKey::Bare`).
//!
//! Const *value* references ([`ObjectConstant::ValueRef`]) are deliberately
//! excluded: a value object's hash is a pure function of its bytes, so a
//! changed value moves the defining (dependency) module's interface hash and
//! therefore the consumer's cache key — the key channel already covers them.
//!
//! Link validation keys the recorded bindings by their rendered `NameKey`
//! string. This is sound because `Display` is **injective over linking-table
//! keys**: an `Item` key is a top-level function `Fqn` (`module::name`, one
//! ident segment — no two collide), a `Bare` key is a uuid-prefixed dispatch
//! symbol, and the two never share a rendering (`core`/`workspace` prefix vs.
//! a uuid). [`LinkState`] additionally poisons any display that ever maps to
//! two hashes, so even a hypothetical collision fails safe.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::compiler::{CompiledModule, MigrationRecord, PrelinkModule};
use crate::disk_store::{BuildManifest, DiskStore, ManifestModule};
use crate::fqn::NameKey;
use crate::module_interface::module_ast_hash;
use crate::module_path::ModulePath;
use crate::module_registry::ModuleRegistry;
use crate::object::{ObjectConstant, ObjectFunction, ObjectRef, StoredObject};

use super::ModuleBuildOutput;

/// Domain separator for a package module's cache key.
const MODULE_KEY_VERSION: &[u8] = b"ambient/cache/module/v2";
/// Domain separator for the core+platform unit key.
const CORE_KEY_VERSION: &[u8] = b"ambient/cache/core/v2";

// ─────────────────────────────────────────────────────────────────────────────
// Cache mode + context
// ─────────────────────────────────────────────────────────────────────────────

/// Whether the build may read cache hits from a previous snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CacheMode {
    /// Consult the snapshot (subject to `AMBIENT_CACHE=off`). The default.
    #[default]
    Auto,
    /// Never read a snapshot — always a full cold build.
    Off,
}

/// The read-side cache for one build: the opened store and the previous
/// builds' manifests, plus the resolved escape-hatch flags.
///
/// A workspace store carries one snapshot pointer per package; the cache
/// reads their **union** (deduplicated by manifest hash), searched in
/// pointer order. Records for one module id agree across manifests when
/// they hit — the key is content-derived — so first-match is sound.
pub(super) struct BuildCache {
    store: Option<DiskStore>,
    prev: Vec<BuildManifest>,
    reads: bool,
    verify: bool,
}

impl BuildCache {
    /// Open the cache for a build. `store_path` is `<root>/.ambient/store`;
    /// `None` (or a store that won't open, or an absent/broken snapshot)
    /// yields a cache that never hits — a plain cold build. `AMBIENT_CACHE=off`
    /// and [`CacheMode::Off`] both disable reads; `AMBIENT_CACHE_VERIFY=1`
    /// enables the recompile-and-compare oracle (unless reads are off).
    pub(super) fn open(store_path: Option<&Path>, mode: CacheMode) -> Self {
        let off = matches!(mode, CacheMode::Off) || env_flag("AMBIENT_CACHE", "off");
        let reads = !off;
        let verify = reads && env_flag("AMBIENT_CACHE_VERIFY", "1");
        let store = store_path.and_then(|p| DiskStore::open(p).ok());
        let prev = if reads {
            store
                .as_ref()
                .and_then(|s| s.current_snapshots().ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        Self {
            store,
            prev,
            reads,
            verify,
        }
    }

    /// Whether the recompile-and-compare oracle is active.
    pub(super) fn verify(&self) -> bool {
        self.verify
    }

    /// The first prior manifest's record for a module, by canonical identity.
    fn prev_module(&self, module_id: &str) -> Option<&ManifestModule> {
        self.prev
            .iter()
            .find_map(|manifest| manifest.modules.iter().find(|m| m.module == module_id))
    }

    /// Whether some prior snapshot's core+platform unit key matches this
    /// build's, so every builtin module can be loaded rather than compiled.
    pub(super) fn core_key_matches(&self, key: [u8; 32]) -> bool {
        self.reads && self.prev.iter().any(|m| m.core_cache_key == key)
    }

    /// Whether the previous snapshot holds a record for `module_id` whose cache
    /// key matches `key`. A match is *necessary but not sufficient* for a warm
    /// hit (link validation may still fail, sending the module to relink or a
    /// full recompile), but it is exactly the signal the check pre-pass keys
    /// off: a key match means every check-level input is unchanged, so the
    /// module's check output is provably identical and needn't be recomputed.
    /// Returns `false` when reads are disabled (a cold build ⇒ every module is a
    /// miss ⇒ the pre-pass checks them all).
    pub(super) fn key_matches(&self, module_id: &str, key: [u8; 32]) -> bool {
        self.reads
            && self
                .prev_module(module_id)
                .is_some_and(|m| m.cache_key == key)
    }

    /// Load every builtin (core + platform) module from the store as one unit.
    /// Populates `all_compiled`, `mfh` (bare linking hashes), and `outputs`,
    /// returning `true` only if *every* module loaded; on any failure it
    /// commits nothing and returns `false` so the caller compiles cold.
    pub(super) fn load_builtins(
        &self,
        registry: &ModuleRegistry,
        paths: &[ModulePath],
        all_compiled: &mut CompiledModule,
        mfh: &mut HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>>,
        outputs: &mut BTreeMap<String, ModuleBuildOutput>,
    ) -> bool {
        let Some(store) = &self.store else {
            return false;
        };
        let mut staged = Vec::new();
        for path in paths {
            // The prelude compiles to nothing and is skipped by the cold
            // pipeline too; loading it is a no-op that would only add an
            // empty entry the cold path never inserts.
            if registry.prelude() == Some(path) {
                continue;
            }
            let module_id = registry.module_id(path).to_string();
            let Some(m) = self.prev_module(&module_id) else {
                return false;
            };
            let Some(loaded) = load_module_from_store(store, m) else {
                return false;
            };
            let bare = bare_function_hashes(&loaded, &module_id);
            staged.push((
                path.clone(),
                module_id,
                loaded,
                ModuleBuildOutput::from_manifest(m),
                bare,
            ));
        }
        for (path, module_id, loaded, output, bare) in staged {
            all_compiled.merge(&loaded);
            mfh.insert(path, bare);
            outputs.insert(module_id, output);
        }
        true
    }

    /// Try to serve a package module from cache: key must match and link
    /// validation must pass against `link`. Returns the materialized module
    /// and its (manifest-reconstructed) build output, or `None` to compile.
    pub(super) fn try_package_module(
        &self,
        module_id: &str,
        cache_key: [u8; 32],
        link: &LinkState,
    ) -> Option<(CompiledModule, ModuleBuildOutput)> {
        if !self.reads {
            return None;
        }
        let store = self.store.as_ref()?;
        let m = self.prev_module(module_id)?;
        if m.cache_key != cache_key {
            return None;
        }
        let loaded = load_module_from_store(store, m)?;
        if !validate_link_hit(m, &loaded, link) {
            return None;
        }
        Some((loaded, ModuleBuildOutput::from_manifest(m)))
    }

    /// Try the **relink fast path** for a module whose full hit failed. The
    /// cache key must still match (so every check-level input is unchanged and
    /// the check output is provably identical); only a dependency's *body* moved
    /// its final content hash, so this module's objects fail link validation
    /// even though nothing it was checked against changed.
    ///
    /// Reloads the module's persisted pre-link symbolic form, remaps the moved
    /// foreign hashes (recorded consumed-links against the current linking
    /// state), and hands the remapped form back for re-finalization. The result
    /// is byte-identical to a cold recompile by construction — with no re-check
    /// and no codegen.
    ///
    /// Returns `None` (fall back to a full recompile) on any obstacle: key
    /// mismatch, no persisted prelink, a missing/corrupt blob, or a consumed
    /// link the current build can no longer resolve to a single hash.
    pub(super) fn try_relink_module(
        &self,
        module_id: &str,
        cache_key: [u8; 32],
        link: &LinkState,
    ) -> Option<PrelinkModule> {
        if !self.reads {
            return None;
        }
        let store = self.store.as_ref()?;
        let m = self.prev_module(module_id)?;
        if m.cache_key != cache_key {
            return None;
        }
        let prelink_hash = blake3::Hash::from_bytes(m.prelink?);
        let mut prelink = store.get_prelink(&prelink_hash).ok()??;
        let remap = build_remap(m, link)?;
        prelink.remap(&remap);
        Some(prelink)
    }
}

/// Build the `old foreign hash → new foreign hash` remap for a relink from the
/// module's recorded consumed-links against the current linking state. Returns
/// `None` (bail to recompile) if any consumed link no longer resolves to a
/// single hash — poisoned (two hashes for one rendering) or absent — or if two
/// links that shared an old hash now disagree on the new one.
fn build_remap(
    m: &ManifestModule,
    link: &LinkState,
) -> Option<HashMap<blake3::Hash, blake3::Hash>> {
    let mut remap: HashMap<blake3::Hash, blake3::Hash> = HashMap::new();
    for (disp, old_bytes) in &m.consumed_links {
        let old = blake3::Hash::from_bytes(*old_bytes);
        let new = link.resolve_display(disp)?;
        match remap.get(&old) {
            Some(existing) if *existing != new => return None,
            _ => {
                remap.insert(old, new);
            }
        }
    }
    Some(remap)
}

fn env_flag(var: &str, on: &str) -> bool {
    std::env::var(var).is_ok_and(|v| v.eq_ignore_ascii_case(on))
}

// ─────────────────────────────────────────────────────────────────────────────
// Cache keys
// ─────────────────────────────────────────────────────────────────────────────

/// The core+platform unit key: a fold of every builtin module's structural
/// AST hash plus the native-contract hash. Deliberately *excludes* the global
/// dispatch-surface hash and user interfaces — builtin compiled objects are
/// invariant to user code (coherence forbids core dispatching on user types),
/// so a user-only edit must never invalidate them.
pub(super) fn core_cache_key(
    registry: &ModuleRegistry,
    builtin_paths: &[ModulePath],
    natives_contract_hash: blake3::Hash,
) -> [u8; 32] {
    let mut entries: Vec<(String, [u8; 32])> = builtin_paths
        .iter()
        .filter_map(|p| {
            registry.get(p).map(|info| {
                (
                    registry.module_id(p).to_string(),
                    *module_ast_hash(&info.module).as_bytes(),
                )
            })
        })
        .collect();
    entries.sort();
    let mut h = blake3::Hasher::new();
    h.update(CORE_KEY_VERSION);
    h.update(natives_contract_hash.as_bytes());
    #[allow(clippy::cast_possible_truncation)]
    for (id, ast) in &entries {
        h.update(&(id.len() as u32).to_le_bytes());
        h.update(id.as_bytes());
        h.update(ast);
    }
    *h.finalize().as_bytes()
}

/// A package module's cache key. `deps` must be sorted by identity.
///
/// Shared with the analysis pipeline (`ambient-analysis`), which keys its
/// per-module check memo on the same discipline so a warm editor analysis is
/// byte-identical to a cold one. Analysis passes a zero `natives_contract_hash`
/// (diagnostics never consume a native's content identity — `extern fn`
/// signatures live in the AST), but reuses the exact fold so the two never
/// drift.
#[must_use]
pub fn module_cache_key(
    resolved_ast_hash: [u8; 32],
    natives_contract_hash: [u8; 32],
    dispatch_surface_hash: [u8; 32],
    deps: &[(String, [u8; 32])],
) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(MODULE_KEY_VERSION);
    h.update(&resolved_ast_hash);
    h.update(&natives_contract_hash);
    h.update(&dispatch_surface_hash);
    #[allow(clippy::cast_possible_truncation)]
    for (id, iface) in deps {
        h.update(&(id.len() as u32).to_le_bytes());
        h.update(id.as_bytes());
        h.update(iface);
    }
    *h.finalize().as_bytes()
}

// ─────────────────────────────────────────────────────────────────────────────
// Linking state (incremental, replaces the O(modules²) rebuild)
// ─────────────────────────────────────────────────────────────────────────────

/// The build's accumulated linking table, extended once per module instead of
/// rebuilt from scratch. Carries both the `NameKey`-keyed table the compiler
/// consumes and a `Display`-keyed view for link validation, kept in lockstep.
#[derive(Default)]
pub(super) struct LinkState {
    table: HashMap<NameKey, blake3::Hash>,
    by_display: HashMap<String, blake3::Hash>,
    poison: HashSet<String>,
}

impl LinkState {
    /// A clone of the `NameKey` linking table, for a module's `imported_hashes`.
    pub(super) fn table(&self) -> HashMap<NameKey, blake3::Hash> {
        self.table.clone()
    }

    /// Consume the state into the final `BuildResult` linking table.
    pub(super) fn into_table(self) -> HashMap<NameKey, blake3::Hash> {
        self.table
    }

    fn insert(&mut self, key: NameKey, hash: blake3::Hash) {
        let disp = key.to_string();
        if self.by_display.get(&disp).is_some_and(|h| *h != hash) {
            self.poison.insert(disp.clone());
        }
        self.by_display.insert(disp, hash);
        self.table.insert(key, hash);
    }

    /// Seed from a `ModulePath`-keyed bare-hash map (the builtin block).
    pub(super) fn seed(
        &mut self,
        mfh: &HashMap<ModulePath, HashMap<Arc<str>, blake3::Hash>>,
        registry: &ModuleRegistry,
    ) {
        for (path, hashes) in mfh {
            for (name, hash) in hashes {
                let key = if name.contains("::") {
                    NameKey::Bare(Arc::clone(name))
                } else {
                    NameKey::Item(registry.fqn(path, &[Arc::clone(name)]))
                };
                self.insert(key, *hash);
            }
        }
    }

    /// Extend with a compiled/loaded module's (already qualified) function
    /// bindings, reconstructing the `NameKey` each would have had.
    pub(super) fn extend_module(
        &mut self,
        qualified_fn_names: &HashMap<Arc<str>, blake3::Hash>,
        module_id: &str,
        path: &ModulePath,
        registry: &ModuleRegistry,
    ) {
        for (qname, hash) in qualified_fn_names {
            self.insert(link_key(qname, module_id, path, registry), *hash);
        }
    }

    /// Whether `disp` currently resolves to exactly `hash` (and is unpoisoned).
    fn resolves(&self, disp: &str, hash: &blake3::Hash) -> bool {
        !self.poison.contains(disp) && self.by_display.get(disp) == Some(hash)
    }

    /// The single hash `disp` currently resolves to, or `None` if it is
    /// poisoned (maps to two hashes, so a remap target would be ambiguous) or
    /// absent (the dependency vanished). The relink remap keys off this.
    fn resolve_display(&self, disp: &str) -> Option<blake3::Hash> {
        if self.poison.contains(disp) {
            return None;
        }
        self.by_display.get(disp).copied()
    }
}

/// Reconstruct the `NameKey` a qualified binding had in the linking table: a
/// module-qualified single-segment name is an `Item` `Fqn`; anything else (a
/// `<uuid>::…` dispatch symbol) stays `Bare` — mirroring `linking_table`.
fn link_key(qname: &str, module_id: &str, path: &ModulePath, registry: &ModuleRegistry) -> NameKey {
    let prefix = format!("{module_id}::");
    if let Some(bare) = qname.strip_prefix(&prefix)
        && !bare.contains("::")
    {
        return NameKey::Item(registry.fqn(path, &[Arc::from(bare)]));
    }
    NameKey::Bare(Arc::from(qname))
}

/// The bare-name → hash map a loaded module contributes to the linking table
/// (`module_function_hashes`): ordinary functions un-qualified to their short
/// name, dispatch symbols kept whole. Consts are excluded (they live in
/// `const_names`, never in the linking table).
fn bare_function_hashes(
    loaded: &CompiledModule,
    module_id: &str,
) -> HashMap<Arc<str>, blake3::Hash> {
    let prefix = format!("{module_id}::");
    loaded
        .function_names
        .iter()
        .map(|(qname, hash)| {
            let bare: Arc<str> = match qname.strip_prefix(&prefix) {
                Some(b) if !b.contains("::") => Arc::from(b),
                _ => Arc::clone(qname),
            };
            (bare, *hash)
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Link validation
// ─────────────────────────────────────────────────────────────────────────────

/// Whether a cached module's recorded consumed links all still resolve to the
/// same hashes, and every external code reference in its objects is covered.
fn validate_link_hit(m: &ManifestModule, loaded: &CompiledModule, link: &LinkState) -> bool {
    for (disp, hash) in &m.consumed_links {
        if !link.resolves(disp, &blake3::Hash::from_bytes(*hash)) {
            return false;
        }
    }
    let recorded: HashSet<blake3::Hash> = m
        .consumed_links
        .iter()
        .map(|(_, h)| blake3::Hash::from_bytes(*h))
        .collect();
    let own = own_hashes(loaded);
    for r in external_code_refs(&loaded.objects) {
        if !own.contains(&r) && !recorded.contains(&r) {
            return false;
        }
    }
    true
}

/// Every hash a module can satisfy from its own products: its object hashes,
/// its materialized function hashes (plain/member), and its const value hashes.
fn own_hashes(module: &CompiledModule) -> HashSet<blake3::Hash> {
    let mut set: HashSet<blake3::Hash> = module.objects.keys().copied().collect();
    set.extend(module.functions.keys().copied());
    set.extend(module.const_names.values().copied());
    set
}

/// Every external (final-hash) *code* reference in a set of objects: callee
/// refs, dispatch-symbol refs, and ability-method `impl_fn`s. Const value
/// refs ([`ObjectConstant::ValueRef`]) are excluded — the interface/key
/// channel covers them.
pub(super) fn external_code_refs(
    objects: &HashMap<blake3::Hash, StoredObject>,
) -> HashSet<blake3::Hash> {
    let mut refs = HashSet::new();
    for obj in objects.values() {
        match obj {
            StoredObject::Plain(f) => collect_fn_refs(f, &mut refs),
            StoredObject::Group(members) => {
                for member in members {
                    collect_fn_refs(&member.function, &mut refs);
                }
            }
            StoredObject::Redirect { .. }
            | StoredObject::Value(_)
            | StoredObject::Native { .. } => {}
        }
    }
    refs
}

fn collect_fn_refs(f: &ObjectFunction, refs: &mut HashSet<blake3::Hash>) {
    for dep in &f.dependencies {
        if let ObjectRef::External(h) = dep {
            refs.insert(*h);
        }
    }
    for c in &f.constants {
        match c {
            ObjectConstant::Ref(ObjectRef::External(h))
            | ObjectConstant::AbilityMethod {
                impl_fn: Some(ObjectRef::External(h)),
                ..
            } => {
                refs.insert(*h);
            }
            _ => {}
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Warm loading + build-output reconstruction
// ─────────────────────────────────────────────────────────────────────────────

/// Materialize a module's runnable [`CompiledModule`] from the store, driven
/// by its manifest record. Every object read self-verifies; a missing or
/// corrupt object (or an unexpected redirect in the canonical set) yields
/// `None` — a silent miss, never an error, so the build always makes progress.
pub(super) fn load_module_from_store(
    store: &DiskStore,
    m: &ManifestModule,
) -> Option<CompiledModule> {
    let mut module = CompiledModule::new();
    module.entry_point = m.entry_point.map(blake3::Hash::from_bytes);

    for obj_hash in &m.objects {
        let h = blake3::Hash::from_bytes(*obj_hash);
        let obj = match store.get_object(&h) {
            Ok(Some(o)) => o,
            // Missing: nothing to heal; the re-persist writes it fresh.
            Ok(None) => return None,
            // Corrupt or undecodable: drop the bad file so the caller's
            // re-persist (which skips paths that already exist) can rewrite
            // the correct bytes — the store self-heals on the next write.
            Err(_) => {
                let _ = std::fs::remove_file(store.object_path(&h));
                return None;
            }
        };
        if matches!(obj, StoredObject::Redirect { .. }) {
            // The manifest only ever records canonical objects.
            return None;
        }
        let materialized = obj.materialize().ok()?;
        let is_group = matches!(&obj, StoredObject::Group(ms) if ms.len() > 1);
        #[allow(clippy::cast_possible_truncation)]
        for (index, (fh, func)) in materialized.into_iter().enumerate() {
            if is_group {
                module.objects.insert(
                    fh,
                    StoredObject::Redirect {
                        group: h,
                        index: index as u32,
                    },
                );
            }
            module.functions.insert(fh, func);
        }
        module.objects.insert(h, obj);
    }

    for (name, hash) in &m.names {
        let h = blake3::Hash::from_bytes(*hash);
        let is_const = matches!(module.objects.get(&h), Some(StoredObject::Value(_)));
        let table = if is_const {
            &mut module.const_names
        } else {
            &mut module.function_names
        };
        table.insert(Arc::from(name.as_str()), h);
    }
    for (name, sig) in &m.signatures {
        module
            .signatures
            .insert(Arc::from(name.as_str()), Arc::from(sig.as_str()));
    }
    module.migrations = m
        .migrations
        .iter()
        .map(|(cell, old, new)| MigrationRecord {
            cell: Arc::from(cell.as_str()),
            old: Arc::from(old.as_str()),
            new: Arc::from(new.as_str()),
        })
        .collect();
    for (h, parent) in &m.lambda_parents {
        module
            .lambda_parents
            .insert(blake3::Hash::from_bytes(*h), Arc::from(parent.as_str()));
    }
    Some(module)
}

impl ModuleBuildOutput {
    /// Reconstruct a build output from a manifest record, byte-for-byte as the
    /// producing cold build recorded it (the manifest is the canonical form).
    pub(super) fn from_manifest(m: &ManifestModule) -> Self {
        Self {
            objects: m
                .objects
                .iter()
                .map(|h| blake3::Hash::from_bytes(*h))
                .collect(),
            names: m
                .names
                .iter()
                .map(|(n, h)| (n.clone(), blake3::Hash::from_bytes(*h)))
                .collect(),
            signatures: m.signatures.iter().cloned().collect(),
            deps: m.deps.clone(),
            consumed_links: m
                .consumed_links
                .iter()
                .map(|(s, h)| (s.clone(), blake3::Hash::from_bytes(*h)))
                .collect(),
            migrations: m
                .migrations
                .iter()
                .map(|(cell, old, new)| MigrationRecord {
                    cell: Arc::from(cell.as_str()),
                    old: Arc::from(old.as_str()),
                    new: Arc::from(new.as_str()),
                })
                .collect(),
            lambda_parents: m
                .lambda_parents
                .iter()
                .map(|(h, p)| (blake3::Hash::from_bytes(*h), p.clone()))
                .collect(),
            entry_point: m.entry_point.map(blake3::Hash::from_bytes),
            cache_key: m.cache_key,
            prelink: m.prelink.map(blake3::Hash::from_bytes),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The per-module build output (compile-time) + verify oracle
// ─────────────────────────────────────────────────────────────────────────────

/// Extract one module's snapshot products from its compiled output. Records
/// the consumed cross-module links by intersecting `imported` with the
/// module's external code references (see the module docs).
pub(super) fn module_output(
    compiled: &CompiledModule,
    deps: Vec<String>,
    imported: &HashMap<NameKey, blake3::Hash>,
    cache_key: [u8; 32],
) -> ModuleBuildOutput {
    let mut objects: Vec<blake3::Hash> = compiled
        .objects
        .iter()
        .filter(|(_, o)| !matches!(o, StoredObject::Redirect { .. }))
        .map(|(h, _)| *h)
        .collect();
    objects.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));

    let names: BTreeMap<String, blake3::Hash> = compiled
        .function_names
        .iter()
        .chain(&compiled.const_names)
        .map(|(name, hash)| (name.to_string(), *hash))
        .collect();
    let signatures: BTreeMap<String, String> = compiled
        .signatures
        .iter()
        .map(|(name, sig)| (name.to_string(), sig.to_string()))
        .collect();
    let mut deps = deps;
    deps.sort_unstable();
    deps.dedup();

    let consumed_links = consumed_links(imported, &compiled.objects);
    let mut lambda_parents: Vec<(blake3::Hash, String)> = compiled
        .lambda_parents
        .iter()
        .map(|(h, p)| (*h, p.to_string()))
        .collect();
    lambda_parents.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

    ModuleBuildOutput {
        objects,
        names,
        signatures,
        deps,
        consumed_links,
        migrations: compiled.migrations.clone(),
        lambda_parents,
        entry_point: compiled.entry_point,
        cache_key,
        // Filled in by the build loop once the fresh prelink blob is hashed;
        // a cache hit reconstructs it from the manifest instead.
        prelink: None,
    }
}

fn consumed_links(
    imported: &HashMap<NameKey, blake3::Hash>,
    objects: &HashMap<blake3::Hash, StoredObject>,
) -> Vec<(String, blake3::Hash)> {
    let refs = external_code_refs(objects);
    let mut out: Vec<(String, blake3::Hash)> = imported
        .iter()
        .filter(|(_, h)| refs.contains(h))
        .map(|(k, h)| (k.to_string(), *h))
        .collect();
    out.sort_by(|a, b| (a.0.as_str(), a.1.as_bytes()).cmp(&(b.0.as_str(), b.1.as_bytes())));
    out
}

/// The verify-mode oracle: assert a cache-loaded module is byte-identical to a
/// freshly compiled one across every persisted channel. Returns a precise diff
/// on mismatch (the caller panics/hard-errors — this is a standing
/// under-invalidation detector, not a recoverable condition).
pub(super) fn assert_equivalent(
    module_id: &str,
    loaded: &ModuleBuildOutput,
    fresh: &ModuleBuildOutput,
) -> Result<(), String> {
    let mut diffs = Vec::new();
    if loaded.objects != fresh.objects {
        diffs.push(format!(
            "objects: cached {} vs fresh {}",
            loaded.objects.len(),
            fresh.objects.len()
        ));
    }
    if loaded.names != fresh.names {
        diffs.push("name bindings differ".to_string());
    }
    if loaded.signatures != fresh.signatures {
        diffs.push("signatures differ".to_string());
    }
    if loaded.migrations != fresh.migrations {
        diffs.push("migrations differ".to_string());
    }
    if loaded.entry_point != fresh.entry_point {
        diffs.push("entry point differs".to_string());
    }
    if diffs.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "AMBIENT_CACHE_VERIFY: stale hit for `{module_id}`: {}",
            diffs.join("; ")
        ))
    }
}

//! Diffing two build snapshots: what changed between two manifests, as a
//! pure data question over their canonical records.
//!
//! Three axes, all deterministically ordered:
//! - **Modules** added/removed/changed. Changed means the resolved-AST hash
//!   moved; a change is further tagged interface-moving (dependents may need
//!   recompiling) vs body-only (the interface hash held).
//! - **Item name bindings**, classified by the *same* rebinding rule the
//!   deploy layer applies at a name swap ([`classify_binding`], mirrored here
//!   because the engine is upstream of `ambient-platform` and cannot import
//!   it): same hash ⇒ unchanged, changed hash with an identical canonical
//!   signature ⇒ rebound, otherwise ⇒ retired-and-fresh; plus added/removed
//!   for names present on only one side.
//! - **Objects** added/removed between the two manifests' object sets, with
//!   byte-size deltas filled from the store when available.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::DiskStore;
use super::snapshot::BuildManifest;

/// One name binding's fate across two snapshots — the rebinding rule in one
/// place, mirroring `ambient_platform::deploy`'s private `classify_name`
/// (pinned identical by a cross-crate agreement test).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingChange {
    /// Bound to the identical hash on both sides.
    Unchanged,
    /// A changed hash whose canonical signature is identical: a rebinding.
    Rebound,
    /// A changed hash whose signature changed (or is missing on either
    /// side): retire-and-fresh.
    Retired,
    /// A name only the newer snapshot carries.
    Added,
}

/// Classify one name's binding against its previous binding — the rebinding
/// rule. `None` for either signature never compares equal, so missing data
/// always classifies as retire-and-fresh (never a silent rebinding). This is
/// the byte-for-byte mirror of the deploy layer's rule; see the module docs.
#[must_use]
pub fn classify_binding(
    prev: Option<(&[u8; 32], Option<&str>)>,
    next: (&[u8; 32], Option<&str>),
) -> BindingChange {
    match prev {
        None => BindingChange::Added,
        Some((prev_hash, _)) if prev_hash == next.0 => BindingChange::Unchanged,
        Some((_, Some(prev_sig))) if next.1 == Some(prev_sig) => BindingChange::Rebound,
        Some(_) => BindingChange::Retired,
    }
}

/// Whether a store name is a content dispatch symbol (`<uuid>::method`)
/// rather than a module-qualified item — excluded from the binding diff,
/// matching the deploy layer (dispatch symbols are content identities, never
/// late-bound names). Mirrors `ambient_platform::deploy::is_dispatch_symbol`.
fn is_dispatch_symbol(name: &str) -> bool {
    name.split("::")
        .next()
        .is_some_and(|head| uuid::Uuid::parse_str(head).is_ok())
}

/// A module that changed between two snapshots (its resolved-AST hash moved).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleChange {
    /// The module's canonical identity.
    pub module: String,
    /// Whether the module's interface hash also moved — a change dependents
    /// can observe (their cache keys shift). `false` means a body-only
    /// change: the observable interface held, only private code / bodies
    /// moved.
    pub interface_changed: bool,
}

/// The module-level diff.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ModuleDiff {
    /// Modules only the newer snapshot has, sorted.
    pub added: Vec<String>,
    /// Modules only the older snapshot had, sorted.
    pub removed: Vec<String>,
    /// Modules present in both whose AST hash moved, sorted by module.
    pub changed: Vec<ModuleChange>,
}

/// The item-binding diff, classified by the deploy rebinding rule.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BindingDiff {
    /// Names only the newer snapshot binds, sorted.
    pub added: Vec<String>,
    /// Names only the older snapshot bound, sorted.
    pub removed: Vec<String>,
    /// Names bound to a new hash with an identical signature, sorted.
    pub rebound: Vec<String>,
    /// Names bound to a new hash with a changed/missing signature
    /// (retire-and-fresh), sorted.
    pub retired: Vec<String>,
}

/// The object-set diff, with byte-size deltas (zero until filled from a
/// store; a missing object counts as zero bytes).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ObjectDiff {
    /// Object hashes only the newer snapshot references, hex, sorted.
    pub added: Vec<String>,
    /// Object hashes only the older snapshot referenced, hex, sorted.
    pub removed: Vec<String>,
    /// Total on-disk bytes of the added objects (best-effort).
    pub added_bytes: u64,
    /// Total on-disk bytes of the removed objects (best-effort).
    pub removed_bytes: u64,
}

/// The full diff between two build snapshots.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SnapshotDiff {
    /// The older snapshot's package name.
    pub from_package: String,
    /// The newer snapshot's package name.
    pub to_package: String,
    /// The module-level diff.
    pub modules: ModuleDiff,
    /// The item name-binding diff.
    pub bindings: BindingDiff,
    /// The object-set diff.
    pub objects: ObjectDiff,
}

impl SnapshotDiff {
    /// True when nothing changed — identical snapshots.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.modules.added.is_empty()
            && self.modules.removed.is_empty()
            && self.modules.changed.is_empty()
            && self.bindings.added.is_empty()
            && self.bindings.removed.is_empty()
            && self.bindings.rebound.is_empty()
            && self.bindings.retired.is_empty()
            && self.objects.added.is_empty()
            && self.objects.removed.is_empty()
    }
}

/// Diff two manifests as pure data — every axis except object byte sizes,
/// which need a store and stay zero here. Deterministically ordered.
#[must_use]
pub fn diff_manifests(from: &BuildManifest, to: &BuildManifest) -> SnapshotDiff {
    SnapshotDiff {
        from_package: from.package_name.clone(),
        to_package: to.package_name.clone(),
        modules: diff_modules(from, to),
        bindings: diff_bindings(from, to),
        objects: diff_objects(from, to),
    }
}

fn diff_modules(from: &BuildManifest, to: &BuildManifest) -> ModuleDiff {
    let from_mods: BTreeMap<&str, (&[u8; 32], &[u8; 32])> = from
        .modules
        .iter()
        .map(|m| (m.module.as_str(), (&m.resolved_ast_hash, &m.interface_hash)))
        .collect();
    let to_mods: BTreeMap<&str, (&[u8; 32], &[u8; 32])> = to
        .modules
        .iter()
        .map(|m| (m.module.as_str(), (&m.resolved_ast_hash, &m.interface_hash)))
        .collect();

    let mut diff = ModuleDiff::default();
    for (name, (ast, iface)) in &to_mods {
        match from_mods.get(name) {
            None => diff.added.push((*name).to_string()),
            Some((from_ast, from_iface)) => {
                if from_ast != ast {
                    diff.changed.push(ModuleChange {
                        module: (*name).to_string(),
                        interface_changed: from_iface != iface,
                    });
                }
            }
        }
    }
    for name in from_mods.keys() {
        if !to_mods.contains_key(name) {
            diff.removed.push((*name).to_string());
        }
    }
    diff.added.sort();
    diff.removed.sort();
    diff.changed.sort_by(|a, b| a.module.cmp(&b.module));
    diff
}

/// Aggregate a manifest's name → (hash, signature) bindings across every
/// module, excluding content dispatch symbols (matching the deploy layer).
fn manifest_bindings(manifest: &BuildManifest) -> BTreeMap<&str, (&[u8; 32], Option<&str>)> {
    let mut out: BTreeMap<&str, (&[u8; 32], Option<&str>)> = BTreeMap::new();
    for module in &manifest.modules {
        let sigs: BTreeMap<&str, &str> = module
            .signatures
            .iter()
            .map(|(n, s)| (n.as_str(), s.as_str()))
            .collect();
        for (name, hash) in &module.names {
            if is_dispatch_symbol(name) {
                continue;
            }
            out.insert(name.as_str(), (hash, sigs.get(name.as_str()).copied()));
        }
    }
    out
}

fn diff_bindings(from: &BuildManifest, to: &BuildManifest) -> BindingDiff {
    let from_bindings = manifest_bindings(from);
    let to_bindings = manifest_bindings(to);

    let mut diff = BindingDiff::default();
    for (name, (hash, sig)) in &to_bindings {
        let prev = from_bindings.get(name).map(|(hash, sig)| (*hash, *sig));
        match classify_binding(prev, (hash, *sig)) {
            BindingChange::Unchanged => {}
            BindingChange::Rebound => diff.rebound.push((*name).to_string()),
            BindingChange::Retired => diff.retired.push((*name).to_string()),
            BindingChange::Added => diff.added.push((*name).to_string()),
        }
    }
    for name in from_bindings.keys() {
        if !to_bindings.contains_key(name) {
            diff.removed.push((*name).to_string());
        }
    }
    diff.added.sort();
    diff.removed.sort();
    diff.rebound.sort();
    diff.retired.sort();
    diff
}

/// Every object hash a manifest references (produced objects, deduplicated).
fn manifest_objects(manifest: &BuildManifest) -> BTreeSet<[u8; 32]> {
    let mut out = BTreeSet::new();
    for module in &manifest.modules {
        out.extend(module.objects.iter().copied());
    }
    out
}

fn diff_objects(from: &BuildManifest, to: &BuildManifest) -> ObjectDiff {
    let from_objects = manifest_objects(from);
    let to_objects = manifest_objects(to);
    let hex = |h: &[u8; 32]| blake3::Hash::from_bytes(*h).to_hex().to_string();

    let mut added: Vec<String> = to_objects.difference(&from_objects).map(hex).collect();
    let mut removed: Vec<String> = from_objects.difference(&to_objects).map(hex).collect();
    // BTreeSet difference yields sorted-by-bytes order; re-sort by hex so the
    // textual order matches the rendered strings.
    added.sort();
    removed.sort();
    ObjectDiff {
        added,
        removed,
        added_bytes: 0,
        removed_bytes: 0,
    }
}

impl DiskStore {
    /// Diff two build snapshots, filling object byte-size deltas from this
    /// store (a missing object counts as zero bytes — the object may have
    /// been collected, or belong to a manifest whose objects were pruned).
    #[must_use]
    pub fn snapshot_diff(&self, from: &BuildManifest, to: &BuildManifest) -> SnapshotDiff {
        let mut diff = diff_manifests(from, to);
        diff.objects.added_bytes = self.sum_object_bytes(&diff.objects.added);
        diff.objects.removed_bytes = self.sum_object_bytes(&diff.objects.removed);
        diff
    }

    /// Total on-disk bytes of the given hex object hashes, skipping any that
    /// don't parse or aren't present.
    fn sum_object_bytes(&self, hex_hashes: &[String]) -> u64 {
        hex_hashes
            .iter()
            .filter_map(|hex| blake3::Hash::from_hex(hex).ok())
            .filter_map(|hash| std::fs::metadata(self.object_path(&hash)).ok())
            .map(|meta| meta.len())
            .sum()
    }
}

//! The user-package check pre-pass.
//!
//! `build_package` separates *check-order* from *link-order*. Type checking a
//! module reads only the registered ASTs + signatures (a compile of module A
//! produces nothing a check of module B consumes), so checking is globally
//! order-independent — the sole ordering constraint is link time. This pass
//! exploits that: it type-checks every cache-*missing* module up front, before
//! the compile/link walk, in a deterministic order.
//!
//! Two properties fall out:
//!
//! - **All check errors surface together.** A cold build with type errors in
//!   several modules reports *all* of them (deterministically ordered by module
//!   identity), rather than stopping at the first in compile order.
//! - **A module is checked exactly once per build.** Key-*match* modules are
//!   never checked here (a warm hit or relink must not re-check); the two rare
//!   recompile fallbacks — verify mode, and a key match that fails both hit and
//!   relink — check lazily in the walk, and those modules were skipped here.
//!
//! The mirror discipline for the core/platform block lives in
//! [`super::pipeline::compile_module_group`], which likewise checks every module
//! before compiling in dependency order.

use std::collections::{BTreeMap, HashMap};

use crate::infer::CheckResult;
use crate::module_interface::ModuleInterfaceSummary;
use crate::module_path::ModulePath;
use crate::module_registry::ModuleRegistry;
use crate::package::Package;

use super::cache::BuildCache;
use super::{BuildError, ModuleTypeErrors, cache, dep_interface_hashes, pipeline};

/// Compute every module's incremental-cache key once, keyed by canonical module
/// id. The key folds only check-level inputs (resolved-AST hash, natives bytes,
/// narrowed dispatch hash, dependency interface hashes) — never [`LinkState`],
/// so it is fully determined before the walk. Both the pre-pass (to decide
/// misses) and the walk (for hit/relink/finish) read this map; the key is never
/// recomputed.
///
/// A `None` value means a dependency interface was absent, so the module can
/// never hit and always recompiles.
///
/// [`LinkState`]: super::cache::LinkState
pub(super) fn compute_cache_keys(
    module_order: &[String],
    paths_by_key: &BTreeMap<String, ModulePath>,
    registry: &ModuleRegistry,
    dep_ids: &BTreeMap<String, Vec<String>>,
    interfaces: &BTreeMap<String, ModuleInterfaceSummary>,
    module_dispatch: &BTreeMap<String, [u8; 32]>,
    natives_bytes: [u8; 32],
) -> HashMap<String, Option<[u8; 32]>> {
    let mut keys = HashMap::with_capacity(module_order.len());
    for module_key in module_order {
        let Some(path) = paths_by_key.get(module_key) else {
            continue;
        };
        let module_id = registry.module_id(path).to_string();
        let key = dep_interface_hashes(dep_ids, &module_id, interfaces).map(|deps| {
            let ast = interfaces
                .get(&module_id)
                .map_or([0u8; 32], |s| *s.resolved_ast_hash.as_bytes());
            let dispatch = module_dispatch
                .get(&module_id)
                .copied()
                .unwrap_or([0u8; 32]);
            cache::module_cache_key(ast, natives_bytes, dispatch, &deps)
        });
        keys.insert(module_id, key);
    }
    keys
}

/// Type-check every cache-missing module in `module_order` up front, keyed by
/// canonical module id for the walk to consume.
///
/// A module misses when its precomputed cache key is absent or does not match a
/// stored entry (a key match that would *later* fail link validation still
/// counts as a match — it is not checked here). Every miss's failure is
/// collected so they surface together as one [`BuildError::TypeCheck`],
/// deterministically ordered by module identity.
///
/// # Errors
///
/// Returns [`BuildError::TypeCheck`] with every failing module's structured
/// errors, or a non-check build error (e.g. a vanished module).
pub(super) fn run(
    pkg: &Package,
    registry: &ModuleRegistry,
    cache: &BuildCache,
    module_order: &[String],
    paths_by_key: &BTreeMap<String, ModulePath>,
    cache_keys: &HashMap<String, Option<[u8; 32]>>,
) -> Result<BTreeMap<String, CheckResult>, BuildError> {
    let mut checked: BTreeMap<String, CheckResult> = BTreeMap::new();
    let mut failures: Vec<ModuleTypeErrors> = Vec::new();

    for module_key in module_order {
        let Some(module_path) = paths_by_key.get(module_key) else {
            continue;
        };
        let module_id = registry.module_id(module_path).to_string();

        // Only misses are checked here. A precomputed `None` key can never hit,
        // so it is always a miss; otherwise the key must match a stored entry.
        let is_miss = match cache_keys.get(&module_id).copied().flatten() {
            Some(key) => !cache.key_matches(&module_id, key),
            None => true,
        };
        if !is_miss {
            continue;
        }

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

        match pipeline::check_loaded_module(module, &file_path, module_path, registry) {
            Ok(check_result) => {
                checked.insert(module_id, check_result);
            }
            // `check_loaded_module` produces a single-element failure vec; drain
            // it into the aggregate so every module's errors surface together.
            Err(BuildError::TypeCheck { failures: mut f }) => failures.append(&mut f),
            Err(other) => return Err(other),
        }
    }

    if !failures.is_empty() {
        // Deterministic rendering order, independent of the compile topo-sort.
        failures.sort_by(|a, b| a.module.cmp(&b.module));
        return Err(BuildError::TypeCheck { failures });
    }
    Ok(checked)
}

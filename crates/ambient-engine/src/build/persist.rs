//! Persisting a completed build to the package-local content-addressed store,
//! and the `build_and_persist` wiring that pairs a build with that write.

use std::path::Path;

use crate::disk_store::{BuildManifest, DiskStore, DiskStoreError};

use super::{BuildError, BuildOptions, BuildResult, ParseFn, build_package};

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

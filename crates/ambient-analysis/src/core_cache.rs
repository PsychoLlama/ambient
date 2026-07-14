//! Materializing the embedded core/platform sources to a versioned,
//! read-only cache directory.
//!
//! Core and platform modules are compiled into the binary via `include_dir!`,
//! so a builtin item resolves fine but has no on-disk file to navigate to. To
//! give goto-definition a real `file://` target this module writes every
//! embedded builtin source to a content-addressed cache directory once, and
//! maps a builtin module path to its file there.
//!
//! This is deliberately in the shared analysis layer, not the LSP: it drags in
//! no editor types, and a future CLI (`ambient resolve-definition`) needs the
//! same file mapping. The LSP owns *when* to materialize and how to wire the
//! resulting URIs; this owns the filesystem mechanics and the versioning.
//!
//! # Versioning
//!
//! The directory name is a blake3 of every embedded module's path + source, so
//! it changes exactly when a builtin source (or the module set) changes. A
//! stale directory can never be reused across a source edit — a new edit lands
//! in a fresh directory, and an existing directory's bytes already match its
//! name. Writes are once (skipped if the directory exists) and staged-then-
//! renamed so a crash or a concurrent writer never leaves a half-populated
//! tree at the final path.

use std::path::{Path, PathBuf};

use ambient_engine::module_path::ModulePath;

/// Env override for the cache base directory. Tests and sandboxes point this
/// at a per-invocation temp dir so materialization stays hermetic; unset in
/// production, where the platform cache dir is used.
pub const CACHE_DIR_ENV: &str = "AMBIENT_CORE_CACHE_DIR";

/// Every embedded builtin (core + platform) module as `(module_path, source)`.
///
/// Both source trees map a file to a module path the same canonical way, and
/// both render a module path back to a file through [`ModulePath::to_file_path`]
/// — so the materialized layout mirrors the module namespace
/// (`core/primitives/number.ab`, `core/system/stdio.ab`).
fn builtin_sources() -> Vec<(ModulePath, &'static str)> {
    let mut modules = ambient_engine::core_library::core_source_modules();
    modules.extend(
        ambient_platform::platform_modules()
            .iter()
            .map(|module| (module.path.clone(), module.source)),
    );
    modules
}

/// The module paths of every embedded builtin, for consumers that index them
/// (the LSP walks these to give core method declarations a navigable URI).
#[must_use]
pub fn builtin_module_paths() -> Vec<ModulePath> {
    builtin_sources()
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

/// Content identity of the embedded builtin source set: a blake3 fold over
/// every module's path + source, order-independent (entries are sorted first).
fn content_key(sources: &[(ModulePath, &'static str)]) -> String {
    let mut entries: Vec<(String, &str)> = sources
        .iter()
        .map(|(path, src)| (path.to_string(), *src))
        .collect();
    entries.sort();

    let mut hasher = blake3::Hasher::new();
    for (path, source) in &entries {
        hasher.update(&(path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update(&(source.len() as u64).to_le_bytes());
        hasher.update(source.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

/// Resolve the cache base directory: an explicit override (the LSP threads a
/// test temp dir here), else the [`CACHE_DIR_ENV`] override, else the platform
/// cache dir (`~/.cache/ambient/core-src` on Linux, honoring `XDG_CACHE_HOME`).
/// `None` only when none of those is available.
fn cache_base(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(base) = explicit {
        return Some(base.to_path_buf());
    }
    if let Some(dir) = std::env::var_os(CACHE_DIR_ENV) {
        return Some(PathBuf::from(dir));
    }
    dirs::cache_dir().map(|dir| dir.join("ambient").join("core-src"))
}

/// The file a builtin `module_path` materializes to under `root`
/// (`core::primitives::number` → `<root>/core/primitives/number.ab`), matching
/// [`ModulePath::to_file_path`] so the writer and the URI mapper agree.
#[must_use]
pub fn builtin_file(root: &Path, module_path: &ModulePath) -> PathBuf {
    root.join(module_path.to_file_path())
}

/// Materialize the embedded builtin sources under a content-addressed
/// directory and return its root, or `None` if no cache location is available.
///
/// Write-once: an existing directory (its name already pins the content) is
/// reused untouched. Files are made read-only best-effort; a filesystem that
/// refuses is not an error.
pub fn materialize(explicit_base: Option<&Path>) -> Option<PathBuf> {
    let sources = builtin_sources();
    let root = cache_base(explicit_base)?.join(content_key(&sources));
    if root.exists() {
        return Some(root);
    }
    match write_tree(&root, &sources) {
        Ok(()) => Some(root),
        // Lost a publish race, or the filesystem refused: reuse the tree if a
        // concurrent writer published it, otherwise give up (no navigation).
        Err(_) => root.exists().then_some(root),
    }
}

/// Write every source into a private staging dir, then atomically rename it to
/// the final content-keyed `root`. The rename is the publish point, so a
/// reader never sees a partial tree.
fn write_tree(root: &Path, sources: &[(ModulePath, &'static str)]) -> std::io::Result<()> {
    let parent = root.parent().unwrap_or(root);
    std::fs::create_dir_all(parent)?;

    // Unique per call (pid + a process-global counter) so concurrent writers —
    // including parallel test threads sharing a base — never share a staging
    // dir and clobber each other mid-write.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let staging = parent.join(format!(".staging-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging); // clear a crashed run's leftovers

    for (module_path, source) in sources {
        let file = builtin_file(&staging, module_path);
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&file, source)?;
        set_readonly(&file);
    }

    match std::fs::rename(&staging, root) {
        Ok(()) => Ok(()),
        Err(err) => {
            // Destination sprang into existence concurrently (rename onto a
            // non-empty dir fails), or a real error: drop staging and report.
            let _ = std::fs::remove_dir_all(&staging);
            Err(err)
        }
    }
}

/// Make a file read-only, best-effort — the cache is a view, not a workspace.
fn set_readonly(file: &Path) {
    if let Ok(meta) = std::fs::metadata(file) {
        let mut perms = meta.permissions();
        perms.set_readonly(true);
        let _ = std::fs::set_permissions(file, perms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn materializes_core_and_platform_sources() {
        let base = TempDir::new().expect("temp dir");
        let root = materialize(Some(base.path())).expect("materialized");

        // Content-addressed subdirectory of the base.
        assert!(root.starts_with(base.path()));
        assert_ne!(root, base.path());

        // A representative core module, a prelude re-export module, an
        // extern-fn module, and a platform module all landed on disk.
        for module in ["core", "core::prelude", "core::exception", "core::system"] {
            let path = ModulePath::from_str_segments(&module.split("::").collect::<Vec<_>>())
                .expect("module path");
            let file = builtin_file(&root, &path);
            assert!(
                file.exists(),
                "missing materialized file for {module}: {file:?}"
            );
        }

        // The `throw` method's declaration text is present and navigable.
        let exception = ModulePath::from_str_segments(&["core", "exception"]).unwrap();
        let source = std::fs::read_to_string(builtin_file(&root, &exception)).unwrap();
        assert!(
            source.contains("fn throw"),
            "exception source not materialized"
        );
    }

    #[test]
    fn write_once_and_content_addressed() {
        let base = TempDir::new().expect("temp dir");
        let first = materialize(Some(base.path())).expect("first");
        let second = materialize(Some(base.path())).expect("second");
        // Same content set → same directory, reused not rewritten.
        assert_eq!(first, second);

        // The directory name is the content key, so any real source set yields
        // a stable, non-empty hex identity.
        let name = first.file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), 64, "expected a blake3 hex directory name");
        assert!(name.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn materialized_files_are_read_only() {
        let base = TempDir::new().expect("temp dir");
        let root = materialize(Some(base.path())).expect("materialized");
        let prelude = ModulePath::from_str_segments(&["core", "prelude"]).unwrap();
        let meta = std::fs::metadata(builtin_file(&root, &prelude)).unwrap();
        assert!(
            meta.permissions().readonly(),
            "materialized file should be read-only"
        );
    }
}

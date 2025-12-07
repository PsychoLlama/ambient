//! Incremental compilation cache.
//!
//! This module provides caching for compiled modules to enable incremental
//! compilation. Only modules whose source has changed are recompiled.

use std::collections::HashMap;

use crate::compiler::CompiledModule;
use crate::module_path::ModulePath;

/// A cache entry for a compiled module.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// Hash of the source file content.
    pub source_hash: blake3::Hash,
    /// The compiled module.
    pub compiled: CompiledModule,
    /// Paths of modules this module depends on.
    pub dependencies: Vec<ModulePath>,
}

/// Cache for compiled modules.
///
/// This enables incremental compilation by tracking which modules have been
/// compiled and only recompiling those whose source has changed.
#[derive(Debug, Default)]
pub struct CompilationCache {
    /// Map from module path to cache entry.
    entries: HashMap<String, CacheEntry>,
}

impl CompilationCache {
    /// Create a new empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if a module is cached and up-to-date.
    ///
    /// Returns `true` if the module is cached and its source hash matches.
    #[must_use]
    pub fn is_up_to_date(&self, path: &ModulePath, source_hash: blake3::Hash) -> bool {
        self.entries
            .get(&path.to_string())
            .is_some_and(|entry| entry.source_hash == source_hash)
    }

    /// Get a cached module if it exists and is up-to-date.
    #[must_use]
    pub fn get(&self, path: &ModulePath, source_hash: blake3::Hash) -> Option<&CacheEntry> {
        let entry = self.entries.get(&path.to_string())?;
        if entry.source_hash == source_hash {
            Some(entry)
        } else {
            None
        }
    }

    /// Get a cached module by path (without checking hash).
    #[must_use]
    pub fn get_by_path(&self, path: &ModulePath) -> Option<&CacheEntry> {
        self.entries.get(&path.to_string())
    }

    /// Insert a compiled module into the cache.
    pub fn insert(
        &mut self,
        path: &ModulePath,
        source_hash: blake3::Hash,
        compiled: CompiledModule,
        dependencies: Vec<ModulePath>,
    ) {
        self.entries.insert(
            path.to_string(),
            CacheEntry {
                source_hash,
                compiled,
                dependencies,
            },
        );
    }

    /// Remove a module from the cache.
    pub fn remove(&mut self, path: &ModulePath) {
        self.entries.remove(&path.to_string());
    }

    /// Invalidate all modules that depend on the given module.
    ///
    /// This should be called when a module changes to ensure dependents
    /// are recompiled.
    pub fn invalidate_dependents(&mut self, changed: &ModulePath) {
        let changed_str = changed.to_string();
        let to_invalidate: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                entry
                    .dependencies
                    .iter()
                    .any(|dep| dep.to_string() == changed_str)
            })
            .map(|(path, _)| path.clone())
            .collect();

        for path in to_invalidate {
            self.entries.remove(&path);
        }
    }

    /// Clear the entire cache.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Get all cached module paths.
    pub fn cached_paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Check if the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get the number of cached modules.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Compute the hash of source content.
#[must_use]
pub fn hash_source(source: &str) -> blake3::Hash {
    blake3::hash(source.as_bytes())
}

/// Compilation result with cache statistics.
#[derive(Debug, Clone)]
pub struct IncrementalCompilationResult {
    /// The main module that was compiled.
    pub main_module: CompiledModule,
    /// All compiled modules (including dependencies).
    pub all_modules: HashMap<String, CompiledModule>,
    /// Number of modules that were compiled.
    pub compiled_count: usize,
    /// Number of modules that were retrieved from cache.
    pub cached_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(segments: &[&str]) -> ModulePath {
        ModulePath::from_str_segments(segments).unwrap()
    }

    #[test]
    fn test_insert_and_get() {
        let mut cache = CompilationCache::new();
        let path = test_path(&["utils"]);
        let hash = hash_source("fn foo() {}");
        let module = CompiledModule::new();

        cache.insert(&path, hash, module.clone(), vec![]);

        assert!(cache.is_up_to_date(&path, hash));
        assert!(cache.get(&path, hash).is_some());
    }

    #[test]
    fn test_stale_cache() {
        let mut cache = CompilationCache::new();
        let path = test_path(&["utils"]);
        let old_hash = hash_source("fn foo() {}");
        let new_hash = hash_source("fn foo(): number { 42 }");
        let module = CompiledModule::new();

        cache.insert(&path, old_hash, module, vec![]);

        assert!(!cache.is_up_to_date(&path, new_hash));
        assert!(cache.get(&path, new_hash).is_none());
    }

    #[test]
    fn test_invalidate_dependents() {
        let mut cache = CompilationCache::new();

        let utils = test_path(&["utils"]);
        let main = test_path(&["main"]);

        let hash = hash_source("content");

        // Main depends on utils
        cache.insert(&utils, hash, CompiledModule::new(), vec![]);
        cache.insert(&main, hash, CompiledModule::new(), vec![utils.clone()]);

        assert!(cache.get_by_path(&main).is_some());

        // Invalidate utils - main should be removed
        cache.invalidate_dependents(&utils);

        assert!(cache.get_by_path(&utils).is_some()); // utils still cached
        assert!(cache.get_by_path(&main).is_none()); // main invalidated
    }
}

//! Core library support for the Ambient language.
//!
//! The core library provides built-in modules that can be imported with
//! `use core.*`. These modules provide essential functionality like
//! list operations, option handling, and basic algorithms.
//!
//! This module only provides source code for core modules. Parsing must
//! be done by the consumer (e.g., CLI or build system) using ambient-parser.

use std::collections::HashMap;
use std::sync::Arc;

use crate::module_path::ModulePath;

/// Error that can occur when loading core library modules.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreLibraryError {
    /// Module not found in core library.
    #[error("core module not found: {0}")]
    ModuleNotFound(String),
}

/// The core library containing built-in modules.
///
/// Provides source code for core modules. Parsing is delegated to the caller.
#[derive(Debug, Default)]
pub struct CoreLibrary {
    /// We don't cache anything in the engine since we can't parse here.
    _marker: (),
}

impl CoreLibrary {
    /// Create a new core library instance.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if a core module exists.
    #[must_use]
    pub fn has_module(path: &[Arc<str>]) -> bool {
        let name = path_to_name(path);
        get_core_modules().contains_key(name.as_str())
    }

    /// Get the source code for a core module.
    ///
    /// # Errors
    ///
    /// Returns an error if the module doesn't exist.
    pub fn get_source(path: &[Arc<str>]) -> Result<&'static str, CoreLibraryError> {
        let name = path_to_name(path);
        get_core_modules()
            .get(name.as_str())
            .copied()
            .ok_or(CoreLibraryError::ModuleNotFound(name))
    }

    /// Get all available core module names.
    #[must_use]
    pub fn available_modules() -> Vec<&'static str> {
        get_core_modules().keys().copied().collect()
    }

    /// Convert a core module path to a `ModulePath`.
    ///
    /// # Panics
    ///
    /// Panics if the path is invalid (should never happen for core modules).
    #[must_use]
    pub fn to_module_path(path: &[Arc<str>]) -> ModulePath {
        // Core modules are prefixed with "core."
        let segments: Vec<Arc<str>> = std::iter::once(Arc::from("core"))
            .chain(path.iter().cloned())
            .collect();
        // This should never fail for valid core module paths
        // Core module paths should always be valid. Use unchecked version.
        // SAFETY: Core module paths are hard-coded and always valid.
        ModulePath::from_segments(segments).unwrap_or_else(|| {
            // This should never happen for valid core module paths
            panic!("Invalid core module path - this is a bug")
        })
    }
}

/// Core modules that participate in the module system, in compilation
/// order. Excludes `traits`: the operator traits (Add, Eq, Ord, ...) are
/// the hardcoded prelude in `TraitRegistry::with_prelude`, and registering
/// a second copy would collide with it.
pub const REGISTERED_CORE_MODULES: &[&str] = &["math", "string", "list"];

/// Parse every registered core module and register it in a module
/// registry under its reserved `core.*` path.
///
/// The engine cannot parse Ambient source itself (the parser depends on
/// the engine), so the caller supplies the parse function.
///
/// # Errors
///
/// Returns the module name and parse error if a core module fails to
/// parse — which is a bug in the embedded sources, not user error.
///
/// # Panics
///
/// Panics if an entry in `REGISTERED_CORE_MODULES` has no embedded
/// source (a compile-time inconsistency in this file).
#[allow(clippy::arc_with_non_send_sync)]
pub fn register_core_modules(
    registry: &mut crate::module_registry::ModuleRegistry,
    parse: impl Fn(&str) -> Result<crate::ast::Module, String>,
) -> Result<Vec<ModulePath>, (String, String)> {
    let sources = get_core_modules();
    let mut paths = Vec::new();
    for name in REGISTERED_CORE_MODULES {
        let source = sources
            .get(name)
            .unwrap_or_else(|| panic!("core module {name} must be embedded"));
        let module = parse(source).map_err(|e| ((*name).to_string(), e))?;
        let path = CoreLibrary::to_module_path(&[Arc::from(*name)]);
        registry.register(&path, Arc::new(module));
        paths.push(path);
    }
    Ok(paths)
}

/// Convert a path to a module name.
fn path_to_name(path: &[Arc<str>]) -> String {
    path.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(".")
}

/// Get the map of core modules and their source code.
fn get_core_modules() -> HashMap<&'static str, &'static str> {
    let mut modules = HashMap::new();
    modules.insert("list", include_str!("core_lib/list.ab"));
    modules.insert("string", include_str!("core_lib/string.ab"));
    modules.insert("math", include_str!("core_lib/math.ab"));
    modules.insert("traits", include_str!("core_lib/traits.ab"));
    modules
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_module() {
        assert!(CoreLibrary::has_module(&[Arc::from("list")]));
        assert!(CoreLibrary::has_module(&[Arc::from("math")]));
        assert!(!CoreLibrary::has_module(&[Arc::from("nonexistent")]));
    }

    #[test]
    fn test_get_source() {
        let source = CoreLibrary::get_source(&[Arc::from("list")]);
        assert!(source.is_ok());
        assert!(source.unwrap().contains("fold"));
    }

    #[test]
    fn test_available_modules() {
        let modules = CoreLibrary::available_modules();
        assert!(modules.contains(&"list"));
        assert!(modules.contains(&"math"));
        assert!(modules.contains(&"string"));
    }

    #[test]
    fn test_to_module_path() {
        let path = CoreLibrary::to_module_path(&[Arc::from("list")]);
        assert_eq!(path.to_string(), "core.list");
    }

    #[test]
    fn test_module_not_found() {
        let result = CoreLibrary::get_source(&[Arc::from("nonexistent")]);
        assert!(matches!(result, Err(CoreLibraryError::ModuleNotFound(_))));
    }
}

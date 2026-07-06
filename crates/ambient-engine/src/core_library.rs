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

/// The intrinsics registered under a core module path, as (name, arity)
/// pairs. Re-exported here so tooling can enumerate a core module's full
/// surface (intrinsics take precedence over compiled functions at the
/// same path).
pub use crate::compiler::intrinsics::get_intrinsics_for_module as intrinsics_for_module;

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
pub const REGISTERED_CORE_MODULES: &[&str] = &[
    "Bool", "Number", "String", "Binary", "List", "Option", "Result", "time",
];

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

        // Intrinsics are items of their core module even though they have
        // no AST declaration (they compile to dedicated opcodes), so they
        // export like any compiled function: `use core::Number::sqrt;` and
        // `use core::Number;` + `Number::sqrt(…)` resolve the same way
        // `core::Number::sqrt(…)` does.
        let segments: Vec<&str> = path.segments().iter().map(AsRef::as_ref).collect();
        let intrinsic_exports = intrinsics_for_module(&segments)
            .into_iter()
            .map(
                |(intrinsic_name, _arity)| crate::module_registry::ExportInfo {
                    name: Arc::from(intrinsic_name),
                    kind: crate::module_registry::ExportKind::Function,
                    is_public: true,
                    re_export_from: None,
                    name_span: crate::ast::Span::default(),
                    doc: None,
                },
            )
            .collect();
        registry.add_exports(&path, intrinsic_exports);

        paths.push(path);
    }
    Ok(paths)
}

/// Parse a declaration-only module (e.g. the `platform` ability bindings
/// interface) and register it at a reserved module path.
///
/// This is the general mechanism behind treating an embedder-supplied
/// module — one containing only declarations, no bodies — as a first-class
/// importable root such as `platform`. Kept parameterized by source string
/// and parse closure so the engine takes no dependency on any particular
/// embedder crate (e.g. `ambient-platform`); the caller supplies both.
///
/// # Errors
///
/// Returns the joined path and parse error if the source fails to parse.
///
/// # Panics
///
/// Panics if `segments` is empty (a caller bug — reserved roots always
/// name at least one segment).
#[allow(clippy::arc_with_non_send_sync)]
pub fn register_declaration_module(
    registry: &mut crate::module_registry::ModuleRegistry,
    segments: &[&str],
    source: &str,
    parse: impl Fn(&str) -> Result<crate::ast::Module, String>,
) -> Result<ModulePath, (String, String)> {
    let joined = segments.join("::");
    let module = parse(source).map_err(|e| (joined.clone(), e))?;
    let path = ModulePath::from_str_segments(segments)
        .unwrap_or_else(|| panic!("declaration module path must be non-empty"));
    registry.register(&path, Arc::new(module));
    Ok(path)
}

/// Convert a path to a module name.
fn path_to_name(path: &[Arc<str>]) -> String {
    path.iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join("::")
}

/// Get the map of core modules and their source code.
fn get_core_modules() -> HashMap<&'static str, &'static str> {
    let mut modules = HashMap::new();
    modules.insert("List", include_str!("core_lib/list.ab"));
    modules.insert("Option", include_str!("core_lib/option.ab"));
    modules.insert("Result", include_str!("core_lib/result.ab"));
    modules.insert("Bool", include_str!("core_lib/bool.ab"));
    modules.insert("Number", include_str!("core_lib/number.ab"));
    modules.insert("String", include_str!("core_lib/string.ab"));
    modules.insert("Binary", include_str!("core_lib/binary.ab"));
    modules.insert("time", include_str!("core_lib/time.ab"));
    modules.insert("traits", include_str!("core_lib/traits.ab"));
    modules
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_module() {
        assert!(CoreLibrary::has_module(&[Arc::from("List")]));
        assert!(CoreLibrary::has_module(&[Arc::from("Number")]));
        assert!(!CoreLibrary::has_module(&[Arc::from("nonexistent")]));
    }

    #[test]
    fn test_get_source() {
        let source = CoreLibrary::get_source(&[Arc::from("List")]);
        assert!(source.is_ok());
        assert!(source.unwrap().contains("fold"));
    }

    #[test]
    fn test_available_modules() {
        let modules = CoreLibrary::available_modules();
        assert!(modules.contains(&"List"));
        assert!(modules.contains(&"Number"));
        assert!(modules.contains(&"String"));
    }

    #[test]
    fn test_to_module_path() {
        let path = CoreLibrary::to_module_path(&[Arc::from("List")]);
        assert_eq!(path.to_string(), "core::List");
    }

    #[test]
    fn test_module_not_found() {
        let result = CoreLibrary::get_source(&[Arc::from("nonexistent")]);
        assert!(matches!(result, Err(CoreLibraryError::ModuleNotFound(_))));
    }

    #[test]
    fn test_time_module_registered() {
        assert!(CoreLibrary::has_module(&[Arc::from("time")]));
        assert!(REGISTERED_CORE_MODULES.contains(&"time"));

        let source = CoreLibrary::get_source(&[Arc::from("time")]).expect("time source");
        assert!(source.contains("struct Duration"));

        let path = CoreLibrary::to_module_path(&[Arc::from("time")]);
        assert_eq!(path.to_string(), "core::time");
    }
}

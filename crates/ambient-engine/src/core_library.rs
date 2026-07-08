//! Core library support for the Ambient language.
//!
//! The core library provides built-in modules importable under the
//! reserved `core::` root (`core::collections::list`,
//! `core::primitives::number`, `core::option`, ...). Its *shape* is
//! defined by the `core_lib/` source tree itself — a directory of `.ab`
//! files plus per-directory `main.ab` modules — not by a hand-maintained
//! list here: [`register_core_modules`] walks the embedded tree and maps
//! every file to a module path through the *same* canonical file↔module
//! mapping ([`ModulePath::from_relative_file_path`]) that user packages
//! use.
//!
//! This module only provides source code for core modules. Parsing must
//! be done by the consumer (e.g., CLI or build system) using ambient-parser.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use include_dir::{Dir, DirEntry, include_dir};

use crate::module_path::ModulePath;

/// The intrinsics registered under a core module path, as (name, arity)
/// pairs. Re-exported here so tooling can enumerate a core module's full
/// surface (intrinsics take precedence over compiled functions at the
/// same path).
pub use crate::compiler::intrinsics::get_intrinsics_for_module as intrinsics_for_module;

/// The embedded core library source tree. Every `.ab` file becomes a
/// module under `core::` — including `traits.ab`, which registers as
/// `core::traits` and is the source of truth for the operator traits.
static CORE_LIB: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/src/core_lib");

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

/// One embedded core module: its full `core::*` path, its source, and
/// whether it is a directory module (backed by a `main.ab`).
struct CoreModule {
    path: ModulePath,
    source: &'static str,
    is_dir_module: bool,
}

/// Every embedded core module, in a deterministic order (by module path).
/// Built once and reused.
fn core_modules() -> &'static [CoreModule] {
    static MODULES: OnceLock<Vec<CoreModule>> = OnceLock::new();
    MODULES.get_or_init(|| {
        let mut modules = Vec::new();
        collect_core_modules(&CORE_LIB, &mut modules);
        modules.sort_by_key(|module| module.path.to_string());
        modules
    })
}

/// Recursively gather the `.ab` files under `dir` into `out`.
fn collect_core_modules(dir: &'static Dir<'static>, out: &mut Vec<CoreModule>) {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(child) => collect_core_modules(child, out),
            DirEntry::File(file) => {
                let path = file.path();
                if path.extension().and_then(|e| e.to_str()) != Some("ab") {
                    continue;
                }
                let (Some((module_path, is_dir_module)), Some(source)) =
                    (core_module_path(path), file.contents_utf8())
                else {
                    continue;
                };
                out.push(CoreModule {
                    path: module_path,
                    source,
                    is_dir_module,
                });
            }
        }
    }
}

/// The `core::*` module path (and directory-module flag) for a file path
/// relative to `core_lib/`, derived through the canonical file↔module
/// mapping so it never forks from how user packages map files to modules.
///
/// The tree's own root `main.ab` is the `core` module itself — a directory
/// module whose members are the top-level core modules.
fn core_module_path(relative: &Path) -> Option<(ModulePath, bool)> {
    let (relative_path, is_dir_module) = ModulePath::from_relative_file_path_with_kind(relative)?;
    if relative_path == ModulePath::root() {
        // `core_lib/main.ab` → the `core` module (a directory module).
        return Some((ModulePath::from_str_segments(&["core"])?, true));
    }
    let mut segments: Vec<Arc<str>> = vec![Arc::from("core")];
    segments.extend(relative_path.segments().iter().cloned());
    Some((ModulePath::from_segments(segments)?, is_dir_module))
}

/// The `core::`-relative name of a core module path (`core::collections::list`
/// → `collections::list`; the `core` root → `""`).
fn core_relative_name(path: &ModulePath) -> String {
    path.segments()
        .iter()
        .skip(1)
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join("::")
}

/// Map from a core module's `::`-joined relative name to its source.
fn core_sources() -> &'static HashMap<String, &'static str> {
    static SOURCES: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    SOURCES.get_or_init(|| {
        core_modules()
            .iter()
            .map(|module| (core_relative_name(&module.path), module.source))
            .collect()
    })
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
        core_sources().contains_key(&path_to_name(path))
    }

    /// Get the source code for a core module.
    ///
    /// # Errors
    ///
    /// Returns an error if the module doesn't exist.
    pub fn get_source(path: &[Arc<str>]) -> Result<&'static str, CoreLibraryError> {
        let name = path_to_name(path);
        core_sources()
            .get(&name)
            .copied()
            .ok_or(CoreLibraryError::ModuleNotFound(name))
    }

    /// Get all available core module names, fully qualified relative to the
    /// `core` root (`collections::list`, `primitives::number`, `Option`,
    /// ...). The `core` root itself is excluded.
    #[must_use]
    pub fn available_modules() -> Vec<String> {
        let mut names: Vec<String> = core_sources()
            .keys()
            .filter(|name| !name.is_empty())
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// Convert a core module path to a `ModulePath`.
    ///
    /// # Panics
    ///
    /// Panics if the path is invalid (should never happen for core modules).
    #[must_use]
    pub fn to_module_path(path: &[Arc<str>]) -> ModulePath {
        // Core modules are prefixed with "core".
        let segments: Vec<Arc<str>> = std::iter::once(Arc::from("core"))
            .chain(path.iter().cloned())
            .collect();
        // SAFETY: Core module paths are hard-coded and always valid.
        ModulePath::from_segments(segments)
            .unwrap_or_else(|| panic!("Invalid core module path - this is a bug"))
    }
}

/// Parse every embedded core module and register it in a module registry
/// under its reserved `core::*` path.
///
/// The engine cannot parse Ambient source itself (the parser depends on
/// the engine), so the caller supplies the parse function. Modules are
/// discovered by walking the embedded `core_lib/` tree; each file's module
/// path comes from the canonical file↔module mapping, so the core
/// hierarchy is defined by the source tree rather than a list here.
///
/// Intrinsics are items of their core module even though they have no AST
/// declaration (they compile to dedicated opcodes), so they export like
/// any compiled function: `use core::primitives::number::sqrt;` and `use
/// core::primitives::number;` + `Number::sqrt(…)` resolve the same way
/// `core::primitives::number::sqrt(…)` does.
///
/// # Errors
///
/// Returns the module name and parse error if a core module fails to
/// parse — which is a bug in the embedded sources, not user error.
#[allow(clippy::arc_with_non_send_sync)]
pub fn register_core_modules(
    registry: &mut crate::module_registry::ModuleRegistry,
    parse: impl Fn(&str) -> Result<crate::ast::Module, String>,
) -> Result<Vec<ModulePath>, (String, String)> {
    // Attach the engine's native bindings for core's `extern fn`
    // declarations. Compiles read them through `ModuleEnv`; the contract
    // (every declaration bound, every binding declared) is verified by
    // `ModuleRegistry::verify_native_contract` once registration completes.
    registry.natives_mut().merge(crate::natives::core_natives());

    let mut paths = Vec::new();
    for module in core_modules() {
        let name = core_relative_name(&module.path);
        let ast = parse(module.source).map_err(|e| (name.clone(), e))?;
        registry.register_module(&module.path, Arc::new(ast), module.is_dir_module);

        let segments: Vec<&str> = module.path.segments().iter().map(AsRef::as_ref).collect();
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
        registry.add_exports(&module.path, intrinsic_exports);

        paths.push(module.path.clone());
    }

    // Default every package's global scope to the core prelude. All three
    // entry points (package build, CLI/LSP platform registry, analysis)
    // funnel through here, so this is the single place the default is set.
    // A future manifest override calls `set_prelude` again afterwards.
    if let Some(prelude) = ModulePath::from_str_segments(&["core", "prelude"]) {
        registry.set_prelude(prelude);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_module() {
        assert!(CoreLibrary::has_module(&[Arc::from("collections::list")]));
        assert!(CoreLibrary::has_module(&[
            Arc::from("primitives"),
            Arc::from("number")
        ]));
        assert!(!CoreLibrary::has_module(&[Arc::from("nonexistent")]));
    }

    #[test]
    fn test_get_source() {
        let source = CoreLibrary::get_source(&[Arc::from("collections::list")]);
        assert!(source.is_ok());
        assert!(source.unwrap().contains("fold"));
    }

    #[test]
    fn test_available_modules() {
        let modules = CoreLibrary::available_modules();
        // Fully qualified relative to the `core` root.
        assert!(modules.contains(&"collections::list".to_string()));
        assert!(modules.contains(&"primitives::number".to_string()));
        assert!(modules.contains(&"primitives::string".to_string()));
        // Namespace parents and top-level modules are present too.
        assert!(modules.contains(&"collections".to_string()));
        assert!(modules.contains(&"option".to_string()));
        // `traits` registers as an ordinary core module now.
        assert!(modules.contains(&"traits".to_string()));
        // The `core` root itself is not.
        assert!(!modules.iter().any(String::is_empty));
    }

    #[test]
    fn test_to_module_path() {
        let path = CoreLibrary::to_module_path(&[Arc::from("collections"), Arc::from("list")]);
        assert_eq!(path.to_string(), "core::collections::list");
    }

    #[test]
    fn test_module_not_found() {
        let result = CoreLibrary::get_source(&[Arc::from("nonexistent")]);
        assert!(matches!(result, Err(CoreLibraryError::ModuleNotFound(_))));
    }

    #[test]
    fn test_time_module_registered() {
        assert!(CoreLibrary::has_module(&[Arc::from("time")]));

        let source = CoreLibrary::get_source(&[Arc::from("time")]).expect("time source");
        assert!(source.contains("struct Duration"));

        let path = CoreLibrary::to_module_path(&[Arc::from("time")]);
        assert_eq!(path.to_string(), "core::time");
    }

    #[test]
    fn test_core_root_is_a_directory_module() {
        // `core_lib/main.ab` maps to the `core` module and is flagged as a
        // directory module so its `pub use self::…` re-exports anchor at
        // `core`.
        let core = core_modules()
            .iter()
            .find(|m| m.path.to_string() == "core")
            .expect("core root module is registered");
        assert!(core.is_dir_module);
    }
}

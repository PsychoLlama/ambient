//! Module registry for cross-module name resolution.
//!
//! The module registry tracks all loaded modules and their exported symbols,
//! enabling cross-module name resolution during type checking and compilation.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{ItemKind, Module, UseKind, UsePrefix};
use crate::module_path::{ImportPrefix, ModulePath, ResolutionError};

/// Error that can occur during module registry operations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// Module not found in the registry.
    #[error("module not found: {0}")]
    ModuleNotFound(String),

    /// Symbol not found in module.
    #[error("symbol `{symbol}` not found in module `{module}`")]
    SymbolNotFound { module: String, symbol: String },

    /// Module path resolution error.
    #[error("path resolution error: {0}")]
    PathResolution(#[from] ResolutionError),

    /// Symbol is not public.
    #[error("symbol `{symbol}` in module `{module}` is not public")]
    NotPublic { module: String, symbol: String },

    /// Core imports are handled separately.
    #[error("core imports are handled separately")]
    CoreHandledSeparately,
}

/// A resolved import - what a name refers to after processing `use` statements.
#[derive(Debug, Clone)]
pub enum ResolvedImport {
    /// The import refers to a module itself (e.g., `use pkg.utils;`)
    Module(ModulePath),
    /// The import refers to a specific symbol from a module.
    Symbol {
        /// The module the symbol comes from.
        from_module: ModulePath,
        /// The kind of symbol.
        export_kind: ExportKind,
    },
}

/// Information about an exported symbol.
#[derive(Debug, Clone)]
pub struct ExportInfo {
    /// The name of the symbol.
    pub name: Arc<str>,
    /// The kind of symbol (function, const, type, enum, ability).
    pub kind: ExportKind,
    /// Whether the symbol is public.
    pub is_public: bool,
    /// If this is a re-export, the original module path.
    pub re_export_from: Option<ModulePath>,
}

/// The kind of exported symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    Function,
    Const,
    TypeAlias,
    Enum,
    EnumVariant,
    Ability,
}

/// Information about a loaded module.
#[derive(Debug, Clone)]
pub struct ModuleInfo {
    /// The module path.
    pub path: ModulePath,
    /// The module's AST.
    pub module: Arc<Module>,
    /// Exported symbols from this module.
    pub exports: HashMap<Arc<str>, ExportInfo>,
    /// Re-exports from other modules (`pub use`).
    pub re_exports: Vec<ReExport>,
}

/// A re-export (`pub use`).
#[derive(Debug, Clone)]
pub struct ReExport {
    /// The prefix of the import.
    pub prefix: UsePrefix,
    /// The module path being re-exported from.
    pub path: Vec<Arc<str>>,
    /// What is re-exported.
    pub kind: UseKind,
}

/// Registry of all loaded modules.
///
/// The registry maintains a map from module paths to their exports,
/// enabling cross-module name resolution.
#[derive(Debug, Default)]
pub struct ModuleRegistry {
    /// Map from module path string to module info.
    modules: HashMap<String, ModuleInfo>,
}

impl ModuleRegistry {
    /// Create a new empty module registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a module in the registry.
    ///
    /// This analyzes the module and extracts its exports.
    pub fn register(&mut self, path: &ModulePath, module: Arc<Module>) {
        let exports = extract_exports(&module);
        let re_exports = extract_re_exports(&module);

        let info = ModuleInfo {
            path: path.clone(),
            module,
            exports,
            re_exports,
        };

        self.modules.insert(path.to_string(), info);
    }

    /// Check if a module is registered.
    #[must_use]
    pub fn contains(&self, path: &ModulePath) -> bool {
        self.modules.contains_key(&path.to_string())
    }

    /// Get a module by its path.
    #[must_use]
    pub fn get(&self, path: &ModulePath) -> Option<&ModuleInfo> {
        self.modules.get(&path.to_string())
    }

    /// Look up a symbol in a module.
    ///
    /// This handles re-exports by following the re-export chain.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The module is not found in the registry
    /// - The symbol is not found in the module
    /// - The symbol is not public
    pub fn lookup_symbol(
        &self,
        module_path: &ModulePath,
        symbol_name: &str,
    ) -> Result<&ExportInfo, RegistryError> {
        let info = self
            .modules
            .get(&module_path.to_string())
            .ok_or_else(|| RegistryError::ModuleNotFound(module_path.to_string()))?;

        // First check direct exports
        if let Some(export) = info.exports.get(symbol_name) {
            if !export.is_public {
                return Err(RegistryError::NotPublic {
                    module: module_path.to_string(),
                    symbol: symbol_name.to_string(),
                });
            }
            return Ok(export);
        }

        // Then check re-exports
        for re_export in &info.re_exports {
            match &re_export.kind {
                UseKind::Module => {
                    // `pub use pkg.other;` - re-exports the module itself, not its contents
                    // The module can be accessed as `other.symbol` through the current module
                }
                UseKind::Glob => {
                    // `pub use pkg.other.*;` - re-exports all public symbols
                    if let Some(target_path) = Self::resolve_import_path(module_path, re_export) {
                        if let Ok(export) = self.lookup_symbol(&target_path, symbol_name) {
                            return Ok(export);
                        }
                    }
                }
                UseKind::Items(items) => {
                    // `pub use pkg.other.{a, b};` - re-exports specific symbols
                    if items.iter().any(|item| item.as_ref() == symbol_name) {
                        if let Some(target_path) = Self::resolve_import_path(module_path, re_export)
                        {
                            if let Ok(export) = self.lookup_symbol(&target_path, symbol_name) {
                                return Ok(export);
                            }
                        }
                    }
                }
            }
        }

        Err(RegistryError::SymbolNotFound {
            module: module_path.to_string(),
            symbol: symbol_name.to_string(),
        })
    }

    /// Get all public exports from a module (for glob imports).
    #[must_use]
    pub fn get_public_exports(&self, module_path: &ModulePath) -> Vec<&ExportInfo> {
        let Some(info) = self.modules.get(&module_path.to_string()) else {
            return Vec::new();
        };

        let mut exports: Vec<&ExportInfo> = info.exports.values().filter(|e| e.is_public).collect();

        // Also include re-exported symbols
        for re_export in &info.re_exports {
            if let UseKind::Glob = &re_export.kind {
                if let Some(target_path) = Self::resolve_import_path(module_path, re_export) {
                    let re_exported = self.get_public_exports(&target_path);
                    exports.extend(re_exported);
                }
            }
        }

        exports
    }

    /// Resolve an import path from a source module.
    fn resolve_import_path(from: &ModulePath, re_export: &ReExport) -> Option<ModulePath> {
        let prefix = match re_export.prefix {
            UsePrefix::Pkg => ImportPrefix::Pkg,
            UsePrefix::Core => return None, // Core imports handled separately
            UsePrefix::Self_ => ImportPrefix::Self_,
            UsePrefix::Super(count) => ImportPrefix::Super(count),
        };

        from.resolve_relative(&prefix, &re_export.path).ok()
    }

    /// Get all registered modules.
    pub fn all_modules(&self) -> impl Iterator<Item = &ModuleInfo> {
        self.modules.values()
    }

    /// Resolve an import from a given module context.
    ///
    /// Given a use statement with a prefix and path, resolve it to a target module path.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The prefix is `Core` (core imports are handled separately)
    /// - The path cannot be resolved relative to the source module
    pub fn resolve_use_path(
        &self,
        from: &ModulePath,
        prefix: &UsePrefix,
        path: &[Arc<str>],
    ) -> Result<ModulePath, RegistryError> {
        let import_prefix = match prefix {
            UsePrefix::Pkg => ImportPrefix::Pkg,
            UsePrefix::Core => return Err(RegistryError::CoreHandledSeparately),
            UsePrefix::Self_ => ImportPrefix::Self_,
            UsePrefix::Super(count) => ImportPrefix::Super(*count),
        };

        from.resolve_relative(&import_prefix, path)
            .map_err(RegistryError::PathResolution)
    }

    /// Get all imported symbols for a module.
    ///
    /// This processes all `use` statements in the module and returns a map
    /// from imported names to their source (module path and export info).
    ///
    /// # Errors
    ///
    /// Returns an error if the module is not found in the registry.
    pub fn resolve_imports(
        &self,
        module_path: &ModulePath,
    ) -> Result<HashMap<Arc<str>, ResolvedImport>, RegistryError> {
        let info = self
            .modules
            .get(&module_path.to_string())
            .ok_or_else(|| RegistryError::ModuleNotFound(module_path.to_string()))?;

        let mut imports = HashMap::new();

        // Process use statements from the module's AST
        for item in &info.module.items {
            if let ItemKind::Use(use_def) = &item.kind {
                // Skip core imports for now (handled separately)
                if matches!(use_def.prefix, UsePrefix::Core) {
                    continue;
                }

                // Resolve the target module path
                let Ok(target_path) =
                    self.resolve_use_path(module_path, &use_def.prefix, &use_def.path)
                else {
                    continue; // Skip unresolvable imports for now
                };

                match &use_def.kind {
                    UseKind::Module => {
                        // Import the module itself as a name
                        // `use pkg.utils;` -> `utils` refers to the module
                        if let Some(last) = use_def.path.last() {
                            imports
                                .insert(last.clone(), ResolvedImport::Module(target_path.clone()));
                        }
                    }
                    UseKind::Glob => {
                        // Import all public symbols from the module
                        for export in self.get_public_exports(&target_path) {
                            imports.insert(
                                export.name.clone(),
                                ResolvedImport::Symbol {
                                    from_module: target_path.clone(),
                                    export_kind: export.kind,
                                },
                            );
                        }
                    }
                    UseKind::Items(items) => {
                        // Import specific items
                        for item_name in items {
                            if let Ok(export) = self.lookup_symbol(&target_path, item_name) {
                                imports.insert(
                                    item_name.clone(),
                                    ResolvedImport::Symbol {
                                        from_module: target_path.clone(),
                                        export_kind: export.kind,
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }

        Ok(imports)
    }
}

/// Extract exports from a module.
fn extract_exports(module: &Module) -> HashMap<Arc<str>, ExportInfo> {
    let mut exports = HashMap::new();

    for item in &module.items {
        let info = match &item.kind {
            ItemKind::Function(f) => Some(ExportInfo {
                name: f.name.clone(),
                kind: ExportKind::Function,
                is_public: f.is_public,
                re_export_from: None,
            }),
            ItemKind::Const(c) => Some(ExportInfo {
                name: c.name.clone(),
                kind: ExportKind::Const,
                is_public: true, // Consts are always public for now
                re_export_from: None,
            }),
            ItemKind::TypeAlias(t) => Some(ExportInfo {
                name: t.name.clone(),
                kind: ExportKind::TypeAlias,
                is_public: true, // Types are always public for now
                re_export_from: None,
            }),
            ItemKind::Enum(e) => {
                // Add the enum itself
                exports.insert(
                    e.name.clone(),
                    ExportInfo {
                        name: e.name.clone(),
                        kind: ExportKind::Enum,
                        is_public: true,
                        re_export_from: None,
                    },
                );

                // Add each variant
                for variant in &e.variants {
                    exports.insert(
                        variant.name.clone(),
                        ExportInfo {
                            name: variant.name.clone(),
                            kind: ExportKind::EnumVariant,
                            is_public: true,
                            re_export_from: None,
                        },
                    );
                }
                None // Already added
            }
            ItemKind::Ability(a) => Some(ExportInfo {
                name: a.name.clone(),
                kind: ExportKind::Ability,
                is_public: true, // Abilities are always public for now
                re_export_from: None,
            }),
            ItemKind::Use(_) => None, // Use statements are not exports
        };

        if let Some(info) = info {
            exports.insert(info.name.clone(), info);
        }
    }

    exports
}

/// Extract re-exports from a module (`pub use` statements).
fn extract_re_exports(module: &Module) -> Vec<ReExport> {
    let mut re_exports = Vec::new();

    for item in &module.items {
        if let ItemKind::Use(use_def) = &item.kind {
            if use_def.is_public {
                re_exports.push(ReExport {
                    prefix: use_def.prefix,
                    path: use_def.path.clone(),
                    kind: use_def.kind.clone(),
                });
            }
        }
    }

    re_exports
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{ConstDef, Expr, FunctionDef, Item, Span};

    fn make_function(name: &str, is_public: bool) -> Item {
        Item::new(
            ItemKind::Function(FunctionDef {
                name: Arc::from(name),
                name_span: Span::default(),
                is_public,
                type_params: vec![],
                params: vec![],
                ret_ty: None,
                abilities: vec![],
                body: Expr::unit(),
            }),
            Span::default(),
        )
    }

    fn make_const(name: &str, value: f64) -> Item {
        use crate::types::Type;
        Item::new(
            ItemKind::Const(ConstDef {
                name: Arc::from(name),
                name_span: Span::default(),
                ty: Type::Number,
                value: Expr::number(value),
            }),
            Span::default(),
        )
    }

    #[test]
    fn test_register_and_lookup() {
        let mut registry = ModuleRegistry::new();

        let module = Arc::new(Module {
            name: Arc::from("utils"),
            items: vec![
                make_function("helper", true),
                make_function("internal", false),
            ],
        });

        let path = ModulePath::from_str_segments(&["utils"]).unwrap();
        registry.register(&path, module);

        // Public function should be found
        let result = registry.lookup_symbol(&path, "helper");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().kind, ExportKind::Function);

        // Private function should error
        let result = registry.lookup_symbol(&path, "internal");
        assert!(matches!(result, Err(RegistryError::NotPublic { .. })));
    }

    #[test]
    fn test_contains() {
        let mut registry = ModuleRegistry::new();

        let module = Arc::new(Module {
            name: Arc::from("test"),
            items: vec![],
        });

        let path = ModulePath::from_str_segments(&["test"]).unwrap();
        assert!(!registry.contains(&path));

        registry.register(&path, module);
        assert!(registry.contains(&path));
    }

    #[test]
    fn test_module_not_found() {
        let registry = ModuleRegistry::new();
        let path = ModulePath::from_str_segments(&["nonexistent"]).unwrap();

        let result = registry.lookup_symbol(&path, "anything");
        assert!(matches!(result, Err(RegistryError::ModuleNotFound(_))));
    }

    #[test]
    fn test_symbol_not_found() {
        let mut registry = ModuleRegistry::new();

        let module = Arc::new(Module {
            name: Arc::from("test"),
            items: vec![make_function("foo", true)],
        });

        let path = ModulePath::from_str_segments(&["test"]).unwrap();
        registry.register(&path, module);

        let result = registry.lookup_symbol(&path, "bar");
        assert!(matches!(result, Err(RegistryError::SymbolNotFound { .. })));
    }

    #[test]
    fn test_get_public_exports() {
        let mut registry = ModuleRegistry::new();

        let module = Arc::new(Module {
            name: Arc::from("test"),
            items: vec![
                make_function("public1", true),
                make_function("public2", true),
                make_function("private", false),
                make_const("PI", 3.14159),
            ],
        });

        let path = ModulePath::from_str_segments(&["test"]).unwrap();
        registry.register(&path, module);

        let exports = registry.get_public_exports(&path);
        assert_eq!(exports.len(), 3); // 2 public functions + 1 const
    }

    #[test]
    fn test_resolve_use_path_pkg() {
        let registry = ModuleRegistry::new();
        let from = ModulePath::from_str_segments(&["main"]).unwrap();
        let path = vec![Arc::from("utils"), Arc::from("format")];

        let resolved = registry.resolve_use_path(&from, &UsePrefix::Pkg, &path);
        assert!(resolved.is_ok());
        assert_eq!(resolved.unwrap().to_string(), "utils.format");
    }

    #[test]
    fn test_resolve_use_path_self() {
        let registry = ModuleRegistry::new();
        let from = ModulePath::from_str_segments(&["utils", "main"]).unwrap();
        let path = vec![Arc::from("sibling")];

        let resolved = registry.resolve_use_path(&from, &UsePrefix::Self_, &path);
        assert!(resolved.is_ok());
        assert_eq!(resolved.unwrap().to_string(), "utils.sibling");
    }

    #[test]
    fn test_resolve_use_path_super() {
        let registry = ModuleRegistry::new();
        let from = ModulePath::from_str_segments(&["a", "b", "c"]).unwrap();
        let path = vec![Arc::from("other")];

        let resolved = registry.resolve_use_path(&from, &UsePrefix::Super(1), &path);
        assert!(resolved.is_ok());
        assert_eq!(resolved.unwrap().to_string(), "a.other");
    }

    #[test]
    fn test_resolve_use_path_core() {
        let registry = ModuleRegistry::new();
        let from = ModulePath::from_str_segments(&["main"]).unwrap();
        let path = vec![Arc::from("list")];

        let resolved = registry.resolve_use_path(&from, &UsePrefix::Core, &path);
        assert!(matches!(
            resolved,
            Err(RegistryError::CoreHandledSeparately)
        ));
    }

    #[test]
    fn test_resolve_imports_items() {
        use crate::ast::{Item, UseDef};

        let mut registry = ModuleRegistry::new();

        // Register the utils module with a helper function
        let utils_module = Arc::new(Module {
            name: Arc::from("utils"),
            items: vec![make_function("helper", true)],
        });
        let utils_path = ModulePath::from_str_segments(&["utils"]).unwrap();
        registry.register(&utils_path, utils_module);

        // Register main module with a use statement
        let main_module = Arc::new(Module {
            name: Arc::from("main"),
            items: vec![Item::new(
                ItemKind::Use(UseDef {
                    is_public: false,
                    prefix: UsePrefix::Pkg,
                    path: vec![Arc::from("utils")],
                    kind: UseKind::Items(vec![Arc::from("helper")]),
                }),
                Span::default(),
            )],
        });
        let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
        registry.register(&main_path, main_module);

        // Resolve imports for main module
        let imports = registry.resolve_imports(&main_path).unwrap();
        assert!(imports.contains_key("helper"));
        match &imports["helper"] {
            ResolvedImport::Symbol {
                from_module,
                export_kind,
            } => {
                assert_eq!(from_module.to_string(), "utils");
                assert_eq!(*export_kind, ExportKind::Function);
            }
            _ => panic!("Expected symbol import"),
        }
    }

    #[test]
    fn test_resolve_imports_glob() {
        use crate::ast::{Item, UseDef};

        let mut registry = ModuleRegistry::new();

        // Register the utils module with multiple functions
        let utils_module = Arc::new(Module {
            name: Arc::from("utils"),
            items: vec![
                make_function("helper1", true),
                make_function("helper2", true),
                make_function("private_fn", false),
            ],
        });
        let utils_path = ModulePath::from_str_segments(&["utils"]).unwrap();
        registry.register(&utils_path, utils_module);

        // Register main module with a glob import
        let main_module = Arc::new(Module {
            name: Arc::from("main"),
            items: vec![Item::new(
                ItemKind::Use(UseDef {
                    is_public: false,
                    prefix: UsePrefix::Pkg,
                    path: vec![Arc::from("utils")],
                    kind: UseKind::Glob,
                }),
                Span::default(),
            )],
        });
        let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
        registry.register(&main_path, main_module);

        // Resolve imports for main module
        let imports = registry.resolve_imports(&main_path).unwrap();
        assert!(imports.contains_key("helper1"));
        assert!(imports.contains_key("helper2"));
        // Private function should not be imported
        assert!(!imports.contains_key("private_fn"));
    }

    #[test]
    fn test_resolve_imports_module() {
        use crate::ast::{Item, UseDef};

        let mut registry = ModuleRegistry::new();

        // Register the utils module
        let utils_module = Arc::new(Module {
            name: Arc::from("utils"),
            items: vec![make_function("helper", true)],
        });
        let utils_path = ModulePath::from_str_segments(&["utils"]).unwrap();
        registry.register(&utils_path, utils_module);

        // Register main module with a module import
        let main_module = Arc::new(Module {
            name: Arc::from("main"),
            items: vec![Item::new(
                ItemKind::Use(UseDef {
                    is_public: false,
                    prefix: UsePrefix::Pkg,
                    path: vec![Arc::from("utils")],
                    kind: UseKind::Module,
                }),
                Span::default(),
            )],
        });
        let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
        registry.register(&main_path, main_module);

        // Resolve imports for main module
        let imports = registry.resolve_imports(&main_path).unwrap();
        // "utils" should be imported as a module reference
        assert!(imports.contains_key("utils"));
        assert!(matches!(&imports["utils"], ResolvedImport::Module(_)));
    }
}

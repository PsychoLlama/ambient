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
}

/// A resolved import - what a name refers to after processing `use` statements.
#[derive(Debug, Clone)]
pub enum ResolvedImport {
    /// The import refers to a module itself (e.g., `use pkg::utils;`)
    Module(ModulePath),
    /// The import refers to a specific symbol from a module.
    Symbol {
        /// The module that defines the symbol, with `pub use` re-export
        /// chains resolved to their origin — which is where the compiled
        /// function hashes live.
        from_module: ModulePath,
        /// The kind of symbol.
        export_kind: ExportKind,
        /// The span of the `use` item that created this import.
        span: crate::ast::Span,
    },
}

/// An import that failed to resolve, with the span of the `use`
/// declaration that caused it.
#[derive(Debug, Clone)]
pub struct ImportError {
    /// Why the import failed.
    pub error: RegistryError,
    /// The span of the `use` item in the importing module.
    pub span: crate::ast::Span,
}

/// The outcome of resolving a module's imports: the bindings that
/// resolved, plus a diagnostic for each import that did not.
#[derive(Debug, Default)]
pub struct ResolvedImports {
    /// Successfully resolved imports, keyed by local name.
    ///
    /// A single name can carry up to two bindings — a module and a symbol
    /// — because modules, values, and types occupy separate namespaces
    /// resolved by syntactic position (`c(...)` is the symbol, `c::foo` the
    /// module). `use a::b::c` binds whichever of those `c` names actually
    /// exist under `a::b`; when both do, both land here and the use site
    /// disambiguates. Within one namespace the last `use` wins.
    pub imports: HashMap<Arc<str>, Vec<ResolvedImport>>,
    /// Imports that failed to resolve.
    pub errors: Vec<ImportError>,
}

impl ResolvedImports {
    /// Bind `import` to `name`, keeping at most one module binding and one
    /// symbol binding per name. A later import of the same namespace shadows
    /// the earlier one; a module and a symbol coexist.
    fn bind(&mut self, name: Arc<str>, import: ResolvedImport) {
        let is_module = matches!(import, ResolvedImport::Module(_));
        let entry = self.imports.entry(name).or_default();
        entry.retain(|existing| matches!(existing, ResolvedImport::Module(_)) != is_module);
        entry.push(import);
    }
}

/// Information about an exported symbol.
#[derive(Debug, Clone)]
pub struct ExportInfo {
    /// The name of the symbol.
    pub name: Arc<str>,
    /// The kind of symbol (function, const, type, enum, variant, ability, trait).
    pub kind: ExportKind,
    /// Whether the symbol is public (declared with `pub`). Enum variants
    /// inherit their enum's visibility.
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
    Trait,
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
    /// This handles re-exports by following the re-export chain. On
    /// success, returns the export along with the module that actually
    /// defines it — for a direct export that is `module_path` itself; for
    /// a `pub use` re-export it is the end of the chain, which is where
    /// the compiled function hashes live.
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
    ) -> Result<(&ExportInfo, ModulePath), RegistryError> {
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
            return Ok((export, module_path.clone()));
        }

        // Then check re-exports. Like the import side, a re-exported name is
        // the final path segment resolved against its parent module — braces
        // are just grouping. `pub use a::b::{x, y}` re-exports symbols `x`,
        // `y` from `a::b`; `pub use a::b::x` re-exports `x` all the same. A
        // bare `pub use a::b` (no parent) re-exports the module itself, not
        // its contents, so it never matches a symbol lookup.
        for re_export in &info.re_exports {
            let matched = match &re_export.kind {
                UseKind::Module => re_export
                    .path
                    .split_last()
                    .filter(|(last, _)| last.as_ref() == symbol_name)
                    .map(|(_, parent)| parent),
                UseKind::Items(items) => items
                    .iter()
                    .any(|item| item.as_ref() == symbol_name)
                    .then_some(re_export.path.as_slice()),
            };

            if let Some(parent) = matched
                && let Some(target_path) = Self::resolve_import_path(module_path, re_export, parent)
                && let Ok(resolved) = self.lookup_symbol(&target_path, symbol_name)
            {
                return Ok(resolved);
            }
        }

        Err(RegistryError::SymbolNotFound {
            module: module_path.to_string(),
            symbol: symbol_name.to_string(),
        })
    }

    /// Get all public exports from a module.
    #[must_use]
    pub fn get_public_exports(&self, module_path: &ModulePath) -> Vec<&ExportInfo> {
        let Some(info) = self.modules.get(&module_path.to_string()) else {
            return Vec::new();
        };

        info.exports.values().filter(|e| e.is_public).collect()
    }

    /// Resolve a re-export's target module path — the module `path` names
    /// relative to `from`. `path` is passed explicitly because the parent of
    /// a non-brace `pub use a::b::c` is `a::b`, not the whole stored path.
    fn resolve_import_path(
        from: &ModulePath,
        re_export: &ReExport,
        path: &[Arc<str>],
    ) -> Option<ModulePath> {
        let prefix = match re_export.prefix {
            UsePrefix::Pkg => ImportPrefix::Pkg,
            // Core imports are handled separately; platform re-exports are
            // unsupported (symmetric with core).
            UsePrefix::Core | UsePrefix::Platform => return None,
            UsePrefix::Self_ => ImportPrefix::Self_,
            UsePrefix::Super(count) => ImportPrefix::Super(count),
        };

        from.resolve_relative(&prefix, path).ok()
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
            UsePrefix::Core => ImportPrefix::Core,
            UsePrefix::Platform => ImportPrefix::Platform,
            UsePrefix::Self_ => ImportPrefix::Self_,
            UsePrefix::Super(count) => ImportPrefix::Super(*count),
        };

        from.resolve_relative(&import_prefix, path)
            .map_err(RegistryError::PathResolution)
    }

    /// Get all imported symbols for a module.
    ///
    /// This processes all `use` statements in the module and returns the
    /// resolved bindings alongside an error for each import that failed —
    /// an unresolvable path, a missing module or symbol, or a private
    /// symbol. Callers surface the errors as diagnostics; a failed import
    /// never binds a name.
    ///
    /// # Errors
    ///
    /// Returns an error if the importing module itself is not in the
    /// registry.
    pub fn resolve_imports(
        &self,
        module_path: &ModulePath,
    ) -> Result<ResolvedImports, RegistryError> {
        let info = self
            .modules
            .get(&module_path.to_string())
            .ok_or_else(|| RegistryError::ModuleNotFound(module_path.to_string()))?;

        let mut resolved = ResolvedImports::default();

        // Process use statements from the module's AST
        for item in &info.module.items {
            let ItemKind::Use(use_def) = &item.kind else {
                continue;
            };

            // Extract just the names from the path (ignoring spans)
            let path_names: Vec<_> = use_def.path.iter().map(|(name, _)| name.clone()).collect();

            // Both forms name a final segment that we resolve against its
            // parent module: `use a::b::c` (Module) and `use a::b::{c}`
            // (Items) mean the same thing. Braces are pure grouping — the
            // difference collapses here, not in the parser. Each candidate
            // is the full path down to the imported name.
            match &use_def.kind {
                UseKind::Module => {
                    self.resolve_use_target(
                        &mut resolved,
                        &use_def.prefix,
                        module_path,
                        &path_names,
                        item.span,
                    );
                }
                UseKind::Items(items) => {
                    for item_name in items {
                        let mut full = path_names.clone();
                        full.push(item_name.clone());
                        self.resolve_use_target(
                            &mut resolved,
                            &use_def.prefix,
                            module_path,
                            &full,
                            item.span,
                        );
                    }
                }
            }
        }

        Ok(resolved)
    }

    /// Resolve one `use` path down to its final segment and bind every
    /// namespace meaning of that segment: the submodule at the full path,
    /// and/or the symbol of that name exported by the parent module.
    ///
    /// This is the single site that unifies the brace and non-brace forms.
    /// `full` is the entire path to the imported name (for `use a::b::c` and
    /// for each item of `use a::b::{c}` alike, it is `[a, b, c]` after prefix
    /// resolution). A name that resolves to neither a submodule nor a symbol
    /// pushes one diagnostic.
    fn resolve_use_target(
        &self,
        resolved: &mut ResolvedImports,
        prefix: &UsePrefix,
        module_path: &ModulePath,
        full: &[Arc<str>],
        span: crate::ast::Span,
    ) {
        // A bare `use self;` has no name to import.
        let Some(local) = full.last().cloned() else {
            return;
        };

        let target = match self.resolve_use_path(module_path, prefix, full) {
            Ok(path) => path,
            Err(error) => {
                resolved.errors.push(ImportError { error, span });
                return;
            }
        };

        // Submodule meaning: the full path itself names a registered module.
        let submodule_bound = self.modules.contains_key(&target.to_string());
        if submodule_bound {
            resolved.bind(local.clone(), ResolvedImport::Module(target.clone()));
        }

        // Symbol meaning: a name exported by the parent module. The parent of
        // a top-level target is the package root module (`main`).
        let symbol_parent = target.parent().unwrap_or_else(ModulePath::root);
        match self.lookup_symbol(&symbol_parent, &local) {
            Ok((export, origin)) => {
                resolved.bind(
                    local,
                    ResolvedImport::Symbol {
                        from_module: origin,
                        export_kind: export.kind,
                        span,
                    },
                );
            }
            Err(error) => {
                // Only a diagnostic if neither namespace bound the name; a
                // successful submodule import means the missing symbol (or
                // missing parent module) was never what the user meant.
                if !submodule_bound {
                    resolved.errors.push(ImportError { error, span });
                }
            }
        }
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
                is_public: c.is_public,
                re_export_from: None,
            }),
            ItemKind::TypeAlias(t) => Some(ExportInfo {
                name: t.name.clone(),
                kind: ExportKind::TypeAlias,
                is_public: t.is_public,
                re_export_from: None,
            }),
            ItemKind::Enum(e) => {
                // Add the enum itself
                exports.insert(
                    e.name.clone(),
                    ExportInfo {
                        name: e.name.clone(),
                        kind: ExportKind::Enum,
                        is_public: e.is_public,
                        re_export_from: None,
                    },
                );

                // Variants inherit the enum's visibility.
                for variant in &e.variants {
                    exports.insert(
                        variant.name.clone(),
                        ExportInfo {
                            name: variant.name.clone(),
                            kind: ExportKind::EnumVariant,
                            is_public: e.is_public,
                            re_export_from: None,
                        },
                    );
                }
                None // Already added
            }
            ItemKind::Ability(a) => Some(ExportInfo {
                name: a.name.clone(),
                kind: ExportKind::Ability,
                is_public: a.is_public,
                re_export_from: None,
            }),
            ItemKind::Trait(t) => Some(ExportInfo {
                name: t.name.clone(),
                kind: ExportKind::Trait,
                is_public: t.is_public,
                re_export_from: None,
            }),
            ItemKind::Use(_) | ItemKind::Impl(_) => None, // Use statements and impls are not exports
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
        if let ItemKind::Use(use_def) = &item.kind
            && use_def.is_public
        {
            re_exports.push(ReExport {
                prefix: use_def.prefix,
                path: use_def.path.iter().map(|(name, _)| name.clone()).collect(),
                kind: use_def.kind.clone(),
            });
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

    fn make_const(name: &str, value: f64, is_public: bool) -> Item {
        use crate::types::Type;
        Item::new(
            ItemKind::Const(ConstDef {
                name: Arc::from(name),
                name_span: Span::default(),
                is_public,
                ty: Type::Number,
                value: Expr::number(value),
            }),
            Span::default(),
        )
    }

    fn make_enum(name: &str, variants: &[&str], is_public: bool) -> Item {
        use crate::ast::{EnumDef, EnumVariant};
        Item::new(
            ItemKind::Enum(EnumDef {
                name: Arc::from(name),
                name_span: Span::default(),
                is_public,
                type_params: vec![],
                variants: variants
                    .iter()
                    .map(|v| EnumVariant {
                        name: Arc::from(*v),
                        payload: None,
                        span: Span::default(),
                    })
                    .collect(),
                uuid: uuid::Uuid::nil(),
            }),
            Span::default(),
        )
    }

    fn make_trait(name: &str, is_public: bool) -> Item {
        use crate::ast::TraitDef;
        Item::new(
            ItemKind::Trait(TraitDef {
                name: Arc::from(name),
                name_span: Span::default(),
                is_public,
                type_params: vec![],
                supertraits: vec![],
                methods: vec![],
            }),
            Span::default(),
        )
    }

    fn make_ability(name: &str, is_public: bool) -> Item {
        use crate::ast::AbilityDef;
        Item::new(
            ItemKind::Ability(AbilityDef {
                name: Arc::from(name),
                name_span: Span::default(),
                is_public,
                dependencies: vec![],
                methods: vec![],
                resolved_id: None,
            }),
            Span::default(),
        )
    }

    #[test]
    fn test_register_and_lookup() {
        let mut registry = ModuleRegistry::new();

        let module = Arc::new(Module {
            name: Arc::from("utils"),
            doc: None,
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
        let (export, origin) = result.unwrap();
        assert_eq!(export.kind, ExportKind::Function);
        assert_eq!(origin, path);

        // Private function should error
        let result = registry.lookup_symbol(&path, "internal");
        assert!(matches!(result, Err(RegistryError::NotPublic { .. })));
    }

    #[test]
    fn test_contains() {
        let mut registry = ModuleRegistry::new();

        let module = Arc::new(Module {
            name: Arc::from("test"),
            doc: None,
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
            doc: None,
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
            doc: None,
            items: vec![
                make_function("public1", true),
                make_function("public2", true),
                make_function("private", false),
                make_const("PI", 3.14159, true),
            ],
        });

        let path = ModulePath::from_str_segments(&["test"]).unwrap();
        registry.register(&path, module);

        let exports = registry.get_public_exports(&path);
        assert_eq!(exports.len(), 3); // 2 public functions + 1 const
    }

    #[test]
    fn private_items_are_not_importable() {
        let mut registry = ModuleRegistry::new();

        let module = Arc::new(Module {
            name: Arc::from("test"),
            doc: None,
            items: vec![
                make_const("SECRET", 42.0, false),
                make_enum("Hidden", &["A", "B"], false),
                make_trait("Sealed", false),
                make_ability("Internal", false),
            ],
        });

        let path = ModulePath::from_str_segments(&["test"]).unwrap();
        registry.register(&path, module);

        for symbol in ["SECRET", "Hidden", "A", "B", "Sealed", "Internal"] {
            let result = registry.lookup_symbol(&path, symbol);
            assert!(
                matches!(result, Err(RegistryError::NotPublic { .. })),
                "expected NotPublic for `{symbol}`, got {result:?}"
            );
        }

        assert!(registry.get_public_exports(&path).is_empty());
    }

    #[test]
    fn public_items_are_importable() {
        let mut registry = ModuleRegistry::new();

        let module = Arc::new(Module {
            name: Arc::from("test"),
            doc: None,
            items: vec![
                make_const("ANSWER", 42.0, true),
                make_enum("Visible", &["Yes"], true),
                make_trait("Open", true),
                make_ability("Exposed", true),
            ],
        });

        let path = ModulePath::from_str_segments(&["test"]).unwrap();
        registry.register(&path, module);

        let cases = [
            ("ANSWER", ExportKind::Const),
            ("Visible", ExportKind::Enum),
            ("Yes", ExportKind::EnumVariant),
            ("Open", ExportKind::Trait),
            ("Exposed", ExportKind::Ability),
        ];
        for (symbol, kind) in cases {
            let (export, _) = registry
                .lookup_symbol(&path, symbol)
                .unwrap_or_else(|e| panic!("expected `{symbol}` to be public, got {e:?}"));
            assert_eq!(export.kind, kind);
        }

        // Enum + variant + const + trait + ability
        assert_eq!(registry.get_public_exports(&path).len(), 5);
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
        let path = vec![Arc::from("List")];

        let resolved = registry
            .resolve_use_path(&from, &UsePrefix::Core, &path)
            .expect("core resolves under the reserved root");
        assert_eq!(resolved.to_string(), "core.List");
    }

    #[test]
    fn test_resolve_imports_items() {
        use crate::ast::{Item, UseDef};

        let mut registry = ModuleRegistry::new();

        // Register the utils module with a helper function
        let utils_module = Arc::new(Module {
            name: Arc::from("utils"),
            doc: None,
            items: vec![make_function("helper", true)],
        });
        let utils_path = ModulePath::from_str_segments(&["utils"]).unwrap();
        registry.register(&utils_path, utils_module);

        // Register main module with a use statement
        let main_module = Arc::new(Module {
            name: Arc::from("main"),
            doc: None,
            items: vec![Item::new(
                ItemKind::Use(UseDef {
                    is_public: false,
                    prefix: UsePrefix::Pkg,
                    path: vec![(Arc::from("utils"), Span::default())],
                    kind: UseKind::Items(vec![Arc::from("helper")]),
                }),
                Span::default(),
            )],
        });
        let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
        registry.register(&main_path, main_module);

        // Resolve imports for main module
        let resolved = registry.resolve_imports(&main_path).unwrap();
        assert!(resolved.errors.is_empty());
        match resolved.imports["helper"].as_slice() {
            [
                ResolvedImport::Symbol {
                    from_module,
                    export_kind,
                    ..
                },
            ] => {
                assert_eq!(from_module.to_string(), "utils");
                assert_eq!(*export_kind, ExportKind::Function);
            }
            other => panic!("Expected a single symbol import, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_imports_module() {
        use crate::ast::{Item, UseDef};

        let mut registry = ModuleRegistry::new();

        // Register the utils module
        let utils_module = Arc::new(Module {
            name: Arc::from("utils"),
            doc: None,
            items: vec![make_function("helper", true)],
        });
        let utils_path = ModulePath::from_str_segments(&["utils"]).unwrap();
        registry.register(&utils_path, utils_module);

        // Register main module with a module import
        let main_module = Arc::new(Module {
            name: Arc::from("main"),
            doc: None,
            items: vec![Item::new(
                ItemKind::Use(UseDef {
                    is_public: false,
                    prefix: UsePrefix::Pkg,
                    path: vec![(Arc::from("utils"), Span::default())],
                    kind: UseKind::Module,
                }),
                Span::default(),
            )],
        });
        let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
        registry.register(&main_path, main_module);

        // Resolve imports for main module
        let resolved = registry.resolve_imports(&main_path).unwrap();
        // "utils" should be imported as a module reference
        assert!(resolved.errors.is_empty());
        assert!(matches!(
            resolved.imports["utils"].as_slice(),
            [ResolvedImport::Module(_)]
        ));
    }

    fn use_module(prefix: UsePrefix, path: &[&str], is_public: bool) -> Item {
        use crate::ast::UseDef;
        Item::new(
            ItemKind::Use(UseDef {
                is_public,
                prefix,
                path: path
                    .iter()
                    .map(|s| (Arc::from(*s), Span::default()))
                    .collect(),
                kind: UseKind::Module,
            }),
            Span::default(),
        )
    }

    /// The non-brace form imports an item just like the brace form:
    /// `use pkg::utils::helper` binds the symbol `helper` when `utils`
    /// exports it, rather than demanding a submodule named `helper`.
    #[test]
    fn non_brace_path_imports_a_symbol() {
        let mut registry = ModuleRegistry::new();

        let utils_path = ModulePath::from_str_segments(&["utils"]).unwrap();
        registry.register(
            &utils_path,
            Arc::new(Module {
                name: Arc::from("utils"),
                doc: None,
                items: vec![make_function("helper", true)],
            }),
        );

        let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
        registry.register(
            &main_path,
            Arc::new(Module {
                name: Arc::from("main"),
                doc: None,
                items: vec![use_module(UsePrefix::Pkg, &["utils", "helper"], false)],
            }),
        );

        let resolved = registry.resolve_imports(&main_path).unwrap();
        assert!(resolved.errors.is_empty(), "errors: {:?}", resolved.errors);
        assert!(
            matches!(
                resolved.imports["helper"].as_slice(),
                [ResolvedImport::Symbol { .. }]
            ),
            "got {:?}",
            resolved.imports.get("helper")
        );
    }

    /// Symmetry the other way: the brace form imports a submodule just like
    /// the non-brace form. `use pkg::a::{b}` binds submodule `a::b`.
    #[test]
    fn brace_form_imports_a_submodule() {
        let mut registry = ModuleRegistry::new();

        // Register submodule `a.b` and its parent `a`.
        for path in [["a"].as_slice(), ["a", "b"].as_slice()] {
            let module_path = ModulePath::from_str_segments(path).unwrap();
            registry.register(
                &module_path,
                Arc::new(Module {
                    name: Arc::from(*path.last().unwrap()),
                    doc: None,
                    items: vec![],
                }),
            );
        }

        let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
        registry.register(
            &main_path,
            Arc::new(Module {
                name: Arc::from("main"),
                doc: None,
                items: vec![use_items(UsePrefix::Pkg, &["a"], &["b"], false)],
            }),
        );

        let resolved = registry.resolve_imports(&main_path).unwrap();
        assert!(resolved.errors.is_empty(), "errors: {:?}", resolved.errors);
        assert!(matches!(
            resolved.imports["b"].as_slice(),
            [ResolvedImport::Module(_)]
        ));
    }

    /// When a name is both a submodule of the parent and a symbol it
    /// exports, `use` binds both — the use site disambiguates by position.
    #[test]
    fn name_that_is_both_submodule_and_symbol_binds_both() {
        let mut registry = ModuleRegistry::new();

        // `a` exports a symbol `b`, and `a.b` is also a submodule.
        registry.register(
            &ModulePath::from_str_segments(&["a"]).unwrap(),
            Arc::new(Module {
                name: Arc::from("a"),
                doc: None,
                items: vec![make_function("b", true)],
            }),
        );
        registry.register(
            &ModulePath::from_str_segments(&["a", "b"]).unwrap(),
            Arc::new(Module {
                name: Arc::from("b"),
                doc: None,
                items: vec![],
            }),
        );

        let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
        registry.register(
            &main_path,
            Arc::new(Module {
                name: Arc::from("main"),
                doc: None,
                items: vec![use_module(UsePrefix::Pkg, &["a", "b"], false)],
            }),
        );

        let resolved = registry.resolve_imports(&main_path).unwrap();
        assert!(resolved.errors.is_empty(), "errors: {:?}", resolved.errors);
        let bindings = &resolved.imports["b"];
        assert_eq!(
            bindings.len(),
            2,
            "expected both bindings, got {bindings:?}"
        );
        assert!(
            bindings
                .iter()
                .any(|b| matches!(b, ResolvedImport::Module(_)))
        );
        assert!(
            bindings
                .iter()
                .any(|b| matches!(b, ResolvedImport::Symbol { .. }))
        );
    }

    /// A non-brace `pub use pkg::origin::helper` re-exports the item just
    /// like the braced form — braces are grouping on the re-export side too.
    #[test]
    fn non_brace_re_export_resolves_to_origin() {
        let mut registry = ModuleRegistry::new();

        let origin_path = ModulePath::from_str_segments(&["origin"]).unwrap();
        registry.register(
            &origin_path,
            Arc::new(Module {
                name: Arc::from("origin"),
                doc: None,
                items: vec![make_function("helper", true)],
            }),
        );

        // facade re-exports `helper` without braces.
        let facade_path = ModulePath::from_str_segments(&["facade"]).unwrap();
        registry.register(
            &facade_path,
            Arc::new(Module {
                name: Arc::from("facade"),
                doc: None,
                items: vec![use_module(UsePrefix::Pkg, &["origin", "helper"], true)],
            }),
        );

        let (_, origin) = registry
            .lookup_symbol(&facade_path, "helper")
            .expect("non-brace re-export should resolve");
        assert_eq!(origin, origin_path);
    }

    fn use_items(prefix: UsePrefix, path: &[&str], items: &[&str], is_public: bool) -> Item {
        use crate::ast::UseDef;
        Item::new(
            ItemKind::Use(UseDef {
                is_public,
                prefix,
                path: path
                    .iter()
                    .map(|s| (Arc::from(*s), Span::default()))
                    .collect(),
                kind: UseKind::Items(items.iter().map(|s| Arc::from(*s)).collect()),
            }),
            Span::default(),
        )
    }

    /// `pub use` chains resolve to the module that defines the symbol,
    /// not the module that re-exports it — that is where compiled hashes
    /// live, so linking depends on it.
    #[test]
    fn re_exports_resolve_to_their_origin() {
        let mut registry = ModuleRegistry::new();

        let origin_path = ModulePath::from_str_segments(&["origin"]).unwrap();
        registry.register(
            &origin_path,
            Arc::new(Module {
                name: Arc::from("origin"),
                doc: None,
                items: vec![make_function("helper", true)],
            }),
        );

        let facade_path = ModulePath::from_str_segments(&["facade"]).unwrap();
        registry.register(
            &facade_path,
            Arc::new(Module {
                name: Arc::from("facade"),
                doc: None,
                items: vec![use_items(UsePrefix::Pkg, &["origin"], &["helper"], true)],
            }),
        );

        let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
        registry.register(
            &main_path,
            Arc::new(Module {
                name: Arc::from("main"),
                doc: None,
                items: vec![use_items(UsePrefix::Pkg, &["facade"], &["helper"], false)],
            }),
        );

        // lookup through the facade lands on the origin
        let (_, origin) = registry.lookup_symbol(&facade_path, "helper").unwrap();
        assert_eq!(origin, origin_path);

        // and resolve_imports records the origin as from_module
        let resolved = registry.resolve_imports(&main_path).unwrap();
        assert!(resolved.errors.is_empty());
        match resolved.imports["helper"].as_slice() {
            [ResolvedImport::Symbol { from_module, .. }] => {
                assert_eq!(*from_module, origin_path);
            }
            other => panic!("Expected a single symbol import, got {other:?}"),
        }
    }

    /// Failed imports surface as errors instead of silently binding
    /// nothing: missing symbols, private symbols, and missing modules.
    #[test]
    fn failed_imports_are_reported() {
        let mut registry = ModuleRegistry::new();

        let utils_path = ModulePath::from_str_segments(&["utils"]).unwrap();
        registry.register(
            &utils_path,
            Arc::new(Module {
                name: Arc::from("utils"),
                doc: None,
                items: vec![make_function("secret", false)],
            }),
        );

        let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
        registry.register(
            &main_path,
            Arc::new(Module {
                name: Arc::from("main"),
                doc: None,
                items: vec![
                    use_items(UsePrefix::Pkg, &["utils"], &["missing"], false),
                    use_items(UsePrefix::Pkg, &["utils"], &["secret"], false),
                    use_items(UsePrefix::Pkg, &["nonexistent"], &["anything"], false),
                ],
            }),
        );

        let resolved = registry.resolve_imports(&main_path).unwrap();
        assert!(resolved.imports.is_empty());
        assert_eq!(resolved.errors.len(), 3);
        assert!(matches!(
            resolved.errors[0].error,
            RegistryError::SymbolNotFound { .. }
        ));
        assert!(matches!(
            resolved.errors[1].error,
            RegistryError::NotPublic { .. }
        ));
        assert!(matches!(
            resolved.errors[2].error,
            RegistryError::ModuleNotFound(_)
        ));
    }
}

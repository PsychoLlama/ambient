//! Module registry for cross-module name resolution.
//!
//! The module registry tracks all loaded modules and their exported symbols,
//! enabling cross-module name resolution during type checking and compilation.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{ItemKind, Module, Span, UseKind, UsePrefix};
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
    /// The span of the defining name in the defining module's source.
    /// Serves go-to-definition; enum variants use their variant span.
    pub name_span: Span,
    /// The item's doc comment, if any (enum variants inherit none).
    pub doc: Option<Arc<str>>,
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
    /// Whether this entry is a directory namespace (no backing file). A
    /// namespace module has no items of its own; its only members are its
    /// child modules. Registering a real module at the same path replaces
    /// the namespace entry (a `foo.ab` file alongside a `foo/` directory
    /// contributes items *and* children).
    pub is_namespace: bool,
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

impl ReExport {
    /// The local name a whole-module re-export exposes: the final path
    /// segment of `pub use a::b::c;`. Item re-exports (`{…}` form) expose
    /// symbol names instead and have no module alias.
    #[must_use]
    pub fn alias(&self) -> Option<&str> {
        match &self.kind {
            UseKind::Module => self.path.last().map(AsRef::as_ref),
            UseKind::Items(_) => None,
        }
    }
}

/// One imported item binding in a module's scope: a local name mapped to
/// the item's canonical identity (defining module + original name), with
/// `pub use` re-export chains already chased to their origin.
#[derive(Debug, Clone)]
pub struct ItemImport {
    /// The defining module.
    pub module: ModulePath,
    /// The item's name in the defining module (differs from the local
    /// name only under an `as` alias).
    pub name: Arc<str>,
    /// The kind of item.
    pub kind: ExportKind,
    /// The span of the `use` item that created this binding.
    pub span: Span,
}

impl ItemImport {
    /// The canonical dotted identity: `<module>.<name>`.
    #[must_use]
    pub fn canonical(&self) -> crate::ast::Resolved {
        crate::ast::Resolved {
            module: self.module.to_string().into(),
            name: Arc::clone(&self.name),
        }
    }
}

/// A module's import scope: every name its `use` items bind, interpreted
/// once, canonically. This is the single source of truth consumed by the
/// resolve pass, the type checker's import channels, and the linker.
///
/// A single local name can bind in several namespaces at once — a module
/// alias, a value, a type, an ability — and the use site's syntactic
/// position picks. Within one namespace the last `use` wins.
#[derive(Debug, Default)]
pub struct ModuleScope {
    /// Module aliases: `use pkg::utils;` binds `utils` → the module path.
    pub modules: HashMap<Arc<str>, ModulePath>,
    /// Item imports by local name. At most one binding per namespace
    /// (values, types, abilities, traits) per name.
    pub items: HashMap<Arc<str>, Vec<ItemImport>>,
    /// Imports that failed to resolve.
    pub errors: Vec<ImportError>,
}

/// The namespace an item kind occupies. Imports shadow within a
/// namespace and coexist across namespaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Namespace {
    Value,
    Type,
    Ability,
    Trait,
}

impl ExportKind {
    /// The namespace this kind of item occupies.
    #[must_use]
    pub fn namespace(self) -> Namespace {
        match self {
            Self::Function | Self::Const | Self::EnumVariant => Namespace::Value,
            Self::TypeAlias | Self::Enum => Namespace::Type,
            Self::Ability => Namespace::Ability,
            Self::Trait => Namespace::Trait,
        }
    }
}

impl ModuleScope {
    /// The item bound to `local` in `ns`, if any.
    #[must_use]
    pub fn item(&self, local: &str, ns: Namespace) -> Option<&ItemImport> {
        self.items
            .get(local)?
            .iter()
            .find(|import| import.kind.namespace() == ns)
    }

    /// The module bound to `local`, if any.
    #[must_use]
    pub fn module(&self, local: &str) -> Option<&ModulePath> {
        self.modules.get(local)
    }

    fn bind_item(&mut self, local: Arc<str>, import: ItemImport) {
        let ns = import.kind.namespace();
        let entry = self.items.entry(local).or_default();
        entry.retain(|existing| existing.kind.namespace() != ns);
        entry.push(import);
    }
}

/// Registry of all loaded modules.
///
/// The registry maintains a map from module paths to their exports,
/// enabling cross-module name resolution. Cloning is cheap relative to
/// building: module ASTs are shared through `Arc`.
#[derive(Debug, Default, Clone)]
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
    /// This analyzes the module and extracts its exports. Every ancestor
    /// directory of the path that has no registered module of its own is
    /// registered as a namespace module, so `src/a/b/c.ab` makes `a` and
    /// `a.b` importable namespaces whose members are their children.
    pub fn register(&mut self, path: &ModulePath, module: Arc<Module>) {
        let exports = extract_exports(&module);
        let re_exports = extract_re_exports(&module);

        let info = ModuleInfo {
            path: path.clone(),
            module,
            exports,
            re_exports,
            is_namespace: false,
        };

        self.modules.insert(path.to_string(), info);
        self.register_namespace_ancestors(path);
    }

    /// Fill in namespace entries for every unregistered ancestor of `path`.
    fn register_namespace_ancestors(&mut self, path: &ModulePath) {
        let mut ancestor = path.parent();
        while let Some(dir) = ancestor {
            let key = dir.to_string();
            if self.modules.contains_key(&key) {
                break;
            }
            self.modules.insert(
                key,
                ModuleInfo {
                    path: dir.clone(),
                    module: Arc::new(Module {
                        name: Arc::from(dir.name()),
                        doc: None,
                        items: Vec::new(),
                    }),
                    exports: HashMap::new(),
                    re_exports: Vec::new(),
                    is_namespace: true,
                },
            );
            ancestor = dir.parent();
        }
    }

    /// Add exports to an already-registered module.
    ///
    /// Core modules use this to expose their intrinsics: an intrinsic has
    /// no AST item (it compiles to a dedicated opcode), but it is still an
    /// item of its module — `use core::math::sqrt;` must resolve exactly
    /// like a compiled function would.
    pub fn add_exports(&mut self, path: &ModulePath, exports: Vec<ExportInfo>) {
        if let Some(info) = self.modules.get_mut(&path.to_string()) {
            for export in exports {
                // Declared items win over injected ones: an intrinsic and a
                // compiled function at the same path prefer the declaration's
                // spans and docs for tooling. (Execution-side precedence is
                // the compiler's concern, not the registry's.)
                info.exports
                    .entry(Arc::clone(&export.name))
                    .or_insert(export);
            }
        }
    }

    /// Resolve one path segment as a child module of `parent`.
    ///
    /// `None` as the parent means the package root: children are the
    /// top-level modules (`src/*.ab` files and `src/*/` directories).
    /// A child is a registered (sub)module, or a whole-module re-export
    /// (`pub use pkg::other::sub;`) in the parent.
    #[must_use]
    pub fn resolve_module_child(
        &self,
        parent: Option<&ModulePath>,
        name: &str,
    ) -> Option<ModulePath> {
        let candidate = match parent {
            Some(dir) => dir.child(name),
            None => ModulePath::from_str_segments(&[name])?,
        };
        if self.modules.contains_key(&candidate.to_string()) {
            return Some(candidate);
        }

        // A module re-export: `pub use pkg::a::b;` in the parent makes `b`
        // a child of the parent for path-walking purposes.
        let parent_info = parent.and_then(|p| self.modules.get(&p.to_string()))?;
        for re_export in &parent_info.re_exports {
            if re_export.alias() != Some(name) {
                continue;
            }
            if let Some(target) =
                Self::resolve_import_path(&parent_info.path, re_export, &re_export.path)
                && self.modules.contains_key(&target.to_string())
            {
                return Some(target);
            }
        }
        None
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
                    .any(|(item, _)| item.as_ref() == symbol_name)
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
            UsePrefix::Core => ImportPrefix::Core,
            UsePrefix::Platform => ImportPrefix::Platform,
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

    /// Build a module's import scope: interpret every `use` item once,
    /// canonically. This is the single site that decides what a `use`
    /// means; everything else (checker, resolve pass, linker,
    /// [`Self::resolve_imports`]) consumes its output.
    ///
    /// Braces are pure grouping: `use a::b::{c}` and `use a::b::c` both
    /// name `c` under `a::b`. Each imported name binds every namespace
    /// meaning of its final segment — the submodule at the full path
    /// and/or the item of that name exported by the parent module. A name
    /// that binds in no namespace pushes one diagnostic.
    #[must_use]
    pub fn build_module_scope(&self, module_path: &ModulePath) -> ModuleScope {
        let mut scope = ModuleScope::default();
        let Some(info) = self.modules.get(&module_path.to_string()) else {
            scope.errors.push(ImportError {
                error: RegistryError::ModuleNotFound(module_path.to_string()),
                span: Span::default(),
            });
            return scope;
        };

        for item in &info.module.items {
            let ItemKind::Use(use_def) = &item.kind else {
                continue;
            };

            let path_names: Vec<_> = use_def.path.iter().map(|(name, _)| name.clone()).collect();

            match &use_def.kind {
                UseKind::Module => {
                    self.bind_use_target(
                        &mut scope,
                        &use_def.prefix,
                        module_path,
                        &path_names,
                        item.span,
                    );
                }
                UseKind::Items(items) => {
                    for (item_name, _) in items {
                        let mut full = path_names.clone();
                        full.push(item_name.clone());
                        self.bind_use_target(
                            &mut scope,
                            &use_def.prefix,
                            module_path,
                            &full,
                            item.span,
                        );
                    }
                }
            }
        }

        scope
    }

    /// Resolve one `use` path down to its final segment and bind every
    /// namespace meaning of that segment into `scope`.
    fn bind_use_target(
        &self,
        scope: &mut ModuleScope,
        prefix: &UsePrefix,
        module_path: &ModulePath,
        full: &[Arc<str>],
        span: Span,
    ) {
        // A bare `use self;` has no name to import.
        let Some(local) = full.last().cloned() else {
            return;
        };

        let target = match self.resolve_use_path(module_path, prefix, full) {
            Ok(path) => path,
            Err(error) => {
                scope.errors.push(ImportError { error, span });
                return;
            }
        };

        // Submodule meaning: the full path itself names a registered module
        // (a file, a directory namespace, or a module re-export in the
        // parent).
        let parent = target.parent();
        let submodule = self.resolve_module_child(parent.as_ref(), &local);
        if let Some(ref submodule_path) = submodule {
            scope
                .modules
                .insert(Arc::clone(&local), submodule_path.clone());
        }

        // Item meaning: a name exported by the parent module. The parent of
        // a top-level target is the package root module (`main`).
        let symbol_parent = parent.unwrap_or_else(ModulePath::root);
        match self.lookup_symbol(&symbol_parent, &local) {
            Ok((export, origin)) => {
                scope.bind_item(
                    Arc::clone(&local),
                    ItemImport {
                        module: origin,
                        name: Arc::clone(&export.name),
                        kind: export.kind,
                        span,
                    },
                );
            }
            Err(error) => {
                // Only a diagnostic if no namespace bound the name; a
                // successful submodule import means the missing symbol (or
                // missing parent module) was never what the user meant.
                if submodule.is_none() {
                    scope.errors.push(ImportError { error, span });
                }
            }
        }
    }

    /// Get all imported symbols for a module.
    ///
    /// An adapter over [`Self::build_module_scope`] for consumers that
    /// want the flat binding view.
    ///
    /// # Errors
    ///
    /// Returns an error if the importing module itself is not in the
    /// registry.
    pub fn resolve_imports(
        &self,
        module_path: &ModulePath,
    ) -> Result<ResolvedImports, RegistryError> {
        if !self.modules.contains_key(&module_path.to_string()) {
            return Err(RegistryError::ModuleNotFound(module_path.to_string()));
        }
        let scope = self.build_module_scope(module_path);

        let mut resolved = ResolvedImports {
            imports: HashMap::new(),
            errors: scope.errors,
        };
        for (local, module) in scope.modules {
            resolved.bind(local, ResolvedImport::Module(module));
        }
        for (local, imports) in scope.items {
            let entry = resolved.imports.entry(local).or_default();
            for import in imports {
                entry.push(ResolvedImport::Symbol {
                    from_module: import.module,
                    export_kind: import.kind,
                    span: import.span,
                });
            }
        }
        Ok(resolved)
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
                name_span: f.name_span,
                doc: item.doc.clone(),
            }),
            ItemKind::Const(c) => Some(ExportInfo {
                name: c.name.clone(),
                kind: ExportKind::Const,
                is_public: c.is_public,
                re_export_from: None,
                name_span: c.name_span,
                doc: item.doc.clone(),
            }),
            ItemKind::TypeAlias(t) => Some(ExportInfo {
                name: t.name.clone(),
                kind: ExportKind::TypeAlias,
                is_public: t.is_public,
                re_export_from: None,
                name_span: t.name_span,
                doc: item.doc.clone(),
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
                        name_span: e.name_span,
                        doc: item.doc.clone(),
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
                            name_span: variant.span,
                            doc: None,
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
                name_span: a.name_span,
                doc: item.doc.clone(),
            }),
            ItemKind::Trait(t) => Some(ExportInfo {
                name: t.name.clone(),
                kind: ExportKind::Trait,
                is_public: t.is_public,
                re_export_from: None,
                name_span: t.name_span,
                doc: item.doc.clone(),
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
                    kind: UseKind::Items(vec![(Arc::from("helper"), Span::default())]),
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
                kind: UseKind::Items(
                    items
                        .iter()
                        .map(|s| (Arc::from(*s), Span::default()))
                        .collect(),
                ),
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

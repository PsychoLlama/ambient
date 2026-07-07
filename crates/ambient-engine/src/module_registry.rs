//! Module registry for cross-module name resolution.
//!
//! The module registry tracks all loaded modules and their exported symbols,
//! enabling cross-module name resolution during type checking and compilation.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{ItemKind, Module, Span, UseDef, UsePrefix};
use crate::fqn::{Fqn, ModuleId};
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

    /// A `Local`-rooted use path whose head never resolved to a module
    /// alias in scope.
    #[error(
        "cannot resolve `{head}`: use paths start with pkg, core, self, super, or a module alias from another `use`"
    )]
    UnresolvedHead { head: String },

    /// A `pub use` re-export rooted at a module alias — re-exports must
    /// use a rooted path so downstream modules can resolve them without
    /// this module's scope.
    #[error("re-export paths must start with pkg, core, self, or super")]
    LocalReExport,
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
    Struct,
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
    /// Whether this module is backed by a `main.ab` (a directory module),
    /// which anchors `self`/`super` at its own path rather than its
    /// parent. Namespace entries are directory-like and set this too.
    pub is_dir_module: bool,
}

/// A re-export (`pub use`), one flattened leaf.
#[derive(Debug, Clone)]
pub struct ReExport {
    /// The prefix of the import.
    pub prefix: UsePrefix,
    /// The path being re-exported.
    pub path: Vec<Arc<str>>,
    /// The exported name when renamed with `as`.
    pub alias: Option<Arc<str>>,
}

impl ReExport {
    /// The local name this re-export exposes: the alias if renamed, else
    /// the final path segment.
    #[must_use]
    pub fn exported_name(&self) -> Option<&str> {
        self.alias
            .as_deref()
            .or_else(|| self.path.last().map(AsRef::as_ref))
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
    /// The canonical [`Fqn`] identity for this import, given the workspace
    /// package name used to scope user modules.
    #[must_use]
    pub fn canonical(&self, workspace: &Arc<str>) -> Fqn {
        Fqn::new(
            ModuleId::from_module_path(&self.module, workspace),
            vec![Arc::clone(&self.name)],
        )
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
    /// Prelude re-exports injected at *lowest* precedence — every module
    /// behaves as though `use prelude::*` were written at its top. Kept
    /// separate from [`Self::items`] on purpose: the resolve pass turns
    /// every `items` binding into an unconditional compile-ordering edge
    /// (its dependency-closure loop), which a prelude binding must not do —
    /// a prelude name only creates an edge when it is actually referenced.
    /// [`Self::item`] consults this after `items`, so an explicit `use`
    /// always shadows the prelude. See [`ModuleRegistry::inject_prelude`].
    pub prelude_items: HashMap<Arc<str>, Vec<ItemImport>>,
    /// Imports that failed to resolve.
    pub errors: Vec<ImportError>,
}

/// The outcome of resolving one `use` leaf's target path.
enum UseTarget {
    /// The absolute module path the leaf names.
    Resolved(ModulePath),
    /// A `Local`-rooted leaf whose head alias isn't bound yet; retried
    /// until the scope reaches a fixed point.
    Waiting,
    /// The leaf cannot resolve.
    Failed(RegistryError),
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
            Self::Struct | Self::TypeAlias | Self::Enum => Namespace::Type,
            Self::Ability => Namespace::Ability,
            Self::Trait => Namespace::Trait,
        }
    }
}

impl ModuleScope {
    /// The item bound to `local` in `ns`, if any. An explicit `use`
    /// (`items`) always wins; the prelude tier is consulted only when no
    /// import occupies the name+namespace.
    #[must_use]
    pub fn item(&self, local: &str, ns: Namespace) -> Option<&ItemImport> {
        self.items
            .get(local)
            .and_then(|imports| imports.iter().find(|import| import.kind.namespace() == ns))
            .or_else(|| {
                self.prelude_items
                    .get(local)?
                    .iter()
                    .find(|import| import.kind.namespace() == ns)
            })
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
#[derive(Debug, Clone)]
pub struct ModuleRegistry {
    /// Map from module path string to module info.
    modules: HashMap<String, ModuleInfo>,
    /// The workspace package name (`ambient.toml` `name`) user modules are
    /// scoped under (`workspace::<name>`). Empty until
    /// [`Self::set_workspace_name`] runs — a consistent placeholder that
    /// keeps every key internally consistent within one build.
    workspace_name: Arc<str>,
    /// The prelude module whose public re-exports are injected into every
    /// module's scope at lowest precedence (see [`Self::inject_prelude`]).
    /// `None` disables injection entirely — the registry-less/single-file
    /// convention. Set to `core::prelude` by `register_core_modules`; a
    /// future per-package manifest override calls [`Self::set_prelude`].
    prelude: Option<ModulePath>,
}

impl Default for ModuleRegistry {
    fn default() -> Self {
        Self {
            modules: HashMap::new(),
            workspace_name: Arc::from(""),
            prelude: None,
        }
    }
}

impl ModuleRegistry {
    /// Create a new empty module registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the workspace package name every user item's [`Fqn`] is scoped
    /// under. Call once per build before resolving/checking so the engine,
    /// LSP, and store all mint identical identities.
    pub fn set_workspace_name(&mut self, name: impl Into<Arc<str>>) {
        self.workspace_name = name.into();
    }

    /// The workspace package name user modules are scoped under.
    #[must_use]
    pub fn workspace_name(&self) -> &Arc<str> {
        &self.workspace_name
    }

    /// Set the prelude module whose public re-exports are injected into
    /// every module's scope at lowest precedence. `register_core_modules`
    /// sets the default (`core::prelude`); a future manifest override calls
    /// this again from `build_package`.
    pub fn set_prelude(&mut self, prelude: ModulePath) {
        self.prelude = Some(prelude);
    }

    /// The prelude module, if injection is enabled.
    #[must_use]
    pub fn prelude(&self) -> Option<&ModulePath> {
        self.prelude.as_ref()
    }

    /// The [`ModuleId`] for a module path under this registry's workspace.
    #[must_use]
    pub fn module_id(&self, path: &ModulePath) -> ModuleId {
        ModuleId::from_module_path(path, &self.workspace_name)
    }

    /// The [`Fqn`] for an item named by `ident` in module `path`, scoped
    /// under this registry's workspace.
    #[must_use]
    pub fn fqn(&self, path: &ModulePath, ident: &[Arc<str>]) -> Fqn {
        Fqn::new(self.module_id(path), ident.to_vec())
    }

    /// Register a module in the registry.
    ///
    /// This analyzes the module and extracts its exports. Every ancestor
    /// directory of the path that has no registered module of its own is
    /// registered as a namespace module, so `src/a/b/c.ab` makes `a` and
    /// `a.b` importable namespaces whose members are their children.
    ///
    /// The module is treated as file-backed (not a directory module); use
    /// [`Self::register_module`] to register a `main.ab` directory module.
    pub fn register(&mut self, path: &ModulePath, module: Arc<Module>) {
        self.register_module(path, module, false);
    }

    /// Register a module, stating whether it is a directory module (backed
    /// by a `main.ab`). See [`ModuleInfo::is_dir_module`].
    pub fn register_module(&mut self, path: &ModulePath, module: Arc<Module>, is_dir_module: bool) {
        let exports = extract_exports(&module);
        let re_exports = extract_re_exports(&module);

        let info = ModuleInfo {
            path: path.clone(),
            module,
            exports,
            re_exports,
            is_namespace: false,
            is_dir_module,
        };

        self.modules.insert(path.to_string(), info);
        self.register_namespace_ancestors(path);
    }

    /// Whether `path` names a directory module (backed by a `main.ab`).
    /// Unregistered paths are treated as file-backed.
    #[must_use]
    fn is_dir_module(&self, path: &ModulePath) -> bool {
        self.modules
            .get(&path.to_string())
            .is_some_and(|info| info.is_dir_module)
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
                    // A namespace is a directory: `self` inside it anchors
                    // at its own path.
                    is_dir_module: true,
                },
            );
            ancestor = dir.parent();
        }
    }

    /// Add exports to an already-registered module.
    ///
    /// Core modules use this to expose their intrinsics: an intrinsic has
    /// no AST item (it compiles to a dedicated opcode), but it is still an
    /// item of its module — `use core::primitives::Number::sqrt;` must resolve exactly
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
            if re_export.exported_name() != Some(name) {
                continue;
            }
            if let Some(target) =
                self.resolve_import_path(&parent_info.path, re_export, &re_export.path)
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

    /// Whether `name` in `module` is a unit struct that denotes a *value* (see
    /// [`crate::ast::StructDef::is_unit_value`]). This answers the cross-module
    /// / imported detection question: a unit struct's bare name is a value
    /// constructor, so value references must resolve to it. An `extern` unit
    /// struct is a type only, so it is excluded.
    #[must_use]
    pub fn is_unit_struct(&self, module: &ModulePath, name: &str) -> bool {
        self.get(module).is_some_and(|info| {
            info.module.items.iter().any(|item| {
                matches!(&item.kind, ItemKind::Struct(s) if s.name.as_ref() == name && s.is_unit_value())
            })
        })
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

        // Then check re-exports. A re-exported name is the final path
        // segment (or its `as` alias) resolved against its parent module:
        // `pub use a::b::x;` re-exports symbol `x` from `a::b`, and
        // `pub use a::b::x as y;` re-exports it as `y`. A single-segment
        // `pub use a;` re-exports the module itself (served by
        // `resolve_module_child`), never a symbol.
        for re_export in &info.re_exports {
            if re_export.exported_name() != Some(symbol_name) {
                continue;
            }
            let Some((original, parent)) = re_export.path.split_last() else {
                continue;
            };
            if parent.is_empty() {
                continue;
            }
            if let Some(parent_path) = self.resolve_import_path(module_path, re_export, parent)
                && let Ok(resolved) = self.lookup_symbol(&parent_path, original)
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
        &self,
        from: &ModulePath,
        re_export: &ReExport,
        path: &[Arc<str>],
    ) -> Option<ModulePath> {
        let prefix = match re_export.prefix {
            UsePrefix::Pkg => ImportPrefix::Pkg,
            UsePrefix::Core => ImportPrefix::Core,
            UsePrefix::Self_ => ImportPrefix::Self_,
            UsePrefix::Super(count) => ImportPrefix::Super(count),
            // Alias-rooted re-exports are rejected at scope building; a
            // stray one resolves to nothing.
            UsePrefix::Local => return None,
        };

        from.resolve_relative(&prefix, path, self.is_dir_module(from))
            .ok()
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
            UsePrefix::Self_ => ImportPrefix::Self_,
            UsePrefix::Super(count) => ImportPrefix::Super(*count),
            UsePrefix::Local => {
                // Alias-rooted paths resolve against a scope, which this
                // path-arithmetic helper doesn't have; `build_module_scope`
                // resolves the head before calling in.
                return Err(RegistryError::UnresolvedHead {
                    head: path.first().map(ToString::to_string).unwrap_or_default(),
                });
            }
        };

        from.resolve_relative(&import_prefix, path, self.is_dir_module(from))
            .map_err(RegistryError::PathResolution)
    }

    /// Build a module's import scope: interpret every `use` item once,
    /// canonically. This is the single site that decides what a `use`
    /// means; everything else (checker, resolve pass, linker,
    /// [`Self::resolve_imports`]) consumes its output.
    ///
    /// Each flattened `use` leaf names an entity by its final segment and
    /// binds every namespace meaning of that segment — the submodule at
    /// the full path and/or the item of that name exported by the parent
    /// module — under the leaf's local name (its `as` alias, else the
    /// segment itself). A leaf that binds in no namespace pushes one
    /// diagnostic.
    ///
    /// `Local`-rooted leaves (`use utils::inner;` where `utils` is a
    /// module alias from another `use`) resolve by fixed point, so their
    /// declaration order doesn't matter.
    #[must_use]
    pub fn build_module_scope(&self, module_path: &ModulePath) -> ModuleScope {
        let Some(info) = self.modules.get(&module_path.to_string()) else {
            let mut scope = ModuleScope::default();
            scope.errors.push(ImportError {
                error: RegistryError::ModuleNotFound(module_path.to_string()),
                span: Span::default(),
            });
            return scope;
        };

        let uses: Vec<(&UseDef, Span)> = info
            .module
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Use(use_def) => Some((use_def, item.span)),
                _ => None,
            })
            .collect();
        let mut scope = self.build_scope_from_uses(module_path, &uses);
        self.inject_prelude(module_path, &mut scope);
        scope
    }

    /// Inject the prelude module's public re-exports into `scope` at lowest
    /// precedence — the mechanism behind "every module behaves as though
    /// `use prelude::*` were written at its top" (the syntax does not
    /// exist; this is the resolver-level equivalent).
    ///
    /// Each re-export leaf is resolved to its defining origin (chasing
    /// `pub use` chains through [`Self::lookup_symbol`], uniform over the
    /// Value/Type/Ability/Trait namespaces) and bound into
    /// [`ModuleScope::prelude_items`] — unless an explicit `use` already
    /// occupies that name+namespace, so imports always shadow the prelude.
    /// The prelude module itself is skipped (it can't import itself), and a
    /// broken re-export is silently dropped (the prelude drift test catches
    /// a genuinely missing name).
    fn inject_prelude(&self, module_path: &ModulePath, scope: &mut ModuleScope) {
        let Some(prelude_path) = &self.prelude else {
            return;
        };
        if prelude_path == module_path {
            return;
        }
        let Some(prelude_info) = self.modules.get(&prelude_path.to_string()) else {
            return;
        };
        for re_export in &prelude_info.re_exports {
            let Some(local) = re_export.exported_name() else {
                continue;
            };
            let Ok((export, origin)) = self.lookup_symbol(prelude_path, local) else {
                continue;
            };
            let ns = export.kind.namespace();
            // An explicit `use` for this name+namespace shadows the prelude.
            if scope
                .items
                .get(local)
                .is_some_and(|imports| imports.iter().any(|i| i.kind.namespace() == ns))
            {
                continue;
            }
            let import = ItemImport {
                module: origin,
                name: Arc::clone(&export.name),
                kind: export.kind,
                span: Span::default(),
            };
            let entry = scope.prelude_items.entry(Arc::from(local)).or_default();
            entry.retain(|existing| existing.kind.namespace() != ns);
            entry.push(import);
        }
    }

    /// The primitive type aliases the prelude contributes to every module:
    /// for each public prelude re-export whose origin is an `extern` unit
    /// struct, the exported name mapped to that struct's nominal type.
    ///
    /// This is the module-system source of the four primitive nominals
    /// (`Bool`/`Number`/`String`/`Binary`). Rather than hardcode
    /// `Type::string()` and friends, they are discovered by walking
    /// `core::prelude`'s re-exports exactly the way [`Self::inject_prelude`]
    /// resolves them — through [`Self::lookup_symbol`], chasing `pub use`
    /// chains to the defining `extern` declaration. Enums (`Option`/`Result`)
    /// and non-re-exported types (`Duration`) are naturally excluded: only a
    /// re-exported `extern` unit struct qualifies.
    ///
    /// Ability resolution seeds these so a primitive named in an ability
    /// signature (`Stdio.out(String)`) resolves to its uuid-carrying nominal
    /// — keeping ability content hashes byte-stable — without the checker
    /// carrying a context-independent `Primitive::from_name` shortcut.
    #[must_use]
    pub fn prelude_type_aliases(&self) -> Vec<(Arc<str>, crate::types::Type)> {
        let mut aliases = Vec::new();
        let Some(prelude_path) = &self.prelude else {
            return aliases;
        };
        let Some(prelude_info) = self.modules.get(&prelude_path.to_string()) else {
            return aliases;
        };
        for re_export in &prelude_info.re_exports {
            let Some(local) = re_export.exported_name() else {
                continue;
            };
            let Ok((export, origin)) = self.lookup_symbol(prelude_path, local) else {
                continue;
            };
            if export.kind != ExportKind::Struct {
                continue;
            }
            let origin_name = Arc::clone(&export.name);
            let Some(origin_info) = self.modules.get(&origin.to_string()) else {
                continue;
            };
            for item in &origin_info.module.items {
                if let ItemKind::Struct(def) = &item.kind
                    && def.name == origin_name
                    && def.is_extern
                    && def.is_unit()
                {
                    aliases.push((Arc::from(local), def.ty.clone()));
                }
            }
        }
        aliases
    }

    /// Interpret a list of `use` items into a scope. Shared by module
    /// scope building and block-scoped `use` resolution.
    #[must_use]
    pub fn build_scope_from_uses(
        &self,
        module_path: &ModulePath,
        uses: &[(&UseDef, Span)],
    ) -> ModuleScope {
        let mut scope = ModuleScope::default();

        // Fixed point: `Local`-rooted leaves wait for the alias they hang
        // off; everything else binds in the first round.
        let mut pending: Vec<&(&UseDef, Span)> = uses.iter().collect();
        loop {
            let mut progress = false;
            let mut still = Vec::new();
            for entry in pending {
                let (use_def, span) = *entry;
                match self.use_target(&scope, module_path, use_def) {
                    UseTarget::Resolved(target) => {
                        self.bind_use_target(&mut scope, use_def, &target, span);
                        progress = true;
                    }
                    UseTarget::Waiting => still.push(entry),
                    UseTarget::Failed(error) => {
                        scope.errors.push(ImportError { error, span });
                        progress = true;
                    }
                }
            }
            pending = still;
            if pending.is_empty() || !progress {
                break;
            }
        }
        for (use_def, span) in pending {
            let head = use_def
                .path
                .first()
                .map(|(name, _)| name.to_string())
                .unwrap_or_default();
            scope.errors.push(ImportError {
                error: RegistryError::UnresolvedHead { head },
                span: *span,
            });
        }

        scope
    }

    /// Resolve one flattened `use` leaf to the absolute module path it
    /// names (the full path including the final segment).
    fn use_target(
        &self,
        scope: &ModuleScope,
        module_path: &ModulePath,
        use_def: &UseDef,
    ) -> UseTarget {
        let path_names: Vec<Arc<str>> = use_def.path.iter().map(|(name, _)| name.clone()).collect();

        if use_def.prefix == UsePrefix::Local {
            if use_def.is_public {
                return UseTarget::Failed(RegistryError::LocalReExport);
            }
            let Some(head) = path_names.first() else {
                return UseTarget::Failed(RegistryError::UnresolvedHead {
                    head: String::new(),
                });
            };
            let Some(base) = scope.module(head) else {
                return UseTarget::Waiting;
            };
            let mut segments = base.segments().to_vec();
            segments.extend(path_names[1..].iter().cloned());
            return match ModulePath::from_segments(segments) {
                Some(target) => UseTarget::Resolved(target),
                None => UseTarget::Failed(RegistryError::UnresolvedHead {
                    head: head.to_string(),
                }),
            };
        }

        match self.resolve_use_path(module_path, &use_def.prefix, &path_names) {
            Ok(target) => UseTarget::Resolved(target),
            Err(error) => UseTarget::Failed(error),
        }
    }

    /// Bind every namespace meaning of a resolved `use` leaf into `scope`.
    pub(crate) fn bind_use_target(
        &self,
        scope: &mut ModuleScope,
        use_def: &UseDef,
        target: &ModulePath,
        span: Span,
    ) {
        // A bare `use self;` has no name to import.
        let Some(local) = use_def.local_name().cloned() else {
            return;
        };
        let original = target.name();

        // Submodule meaning: the full path itself names a registered module
        // (a file, a directory namespace, or a module re-export in the
        // parent).
        let parent = target.parent();
        let submodule = self.resolve_module_child(parent.as_ref(), original);
        if let Some(ref submodule_path) = submodule {
            scope
                .modules
                .insert(Arc::clone(&local), submodule_path.clone());
        }

        // Item meaning: a name exported by the parent module. The parent of
        // a top-level target is the package root module (`main`).
        let symbol_parent = parent.unwrap_or_else(ModulePath::root);
        match self.lookup_symbol(&symbol_parent, original) {
            Ok((export, origin)) => {
                scope.bind_item(
                    local,
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
        // Prelude re-exports are ordinary imports to the checker's
        // consumers (`register_imported_enums`, `retain_imported_type_aliases`),
        // at lowest precedence. Enum *variants* are excluded: the resolve
        // pass reads them straight off `ModuleScope::prelude_items` (so bare
        // `Some`/`None` still resolve), and surfacing them here would trip
        // `build_import_env`'s "import its enum instead" guard for a name
        // the user never wrote a `use` for.
        for (local, imports) in scope.prelude_items {
            let entry = resolved.imports.entry(local).or_default();
            for import in imports {
                if import.kind == ExportKind::EnumVariant {
                    continue;
                }
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
            ItemKind::Struct(s) => Some(ExportInfo {
                name: s.name.clone(),
                kind: ExportKind::Struct,
                is_public: s.is_public,
                re_export_from: None,
                name_span: s.name_span,
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
                alias: use_def.alias.as_ref().map(|(name, _)| name.clone()),
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
                id: 0,
                name: Arc::from(name),
                name_span: Span::default(),
                is_public,
                ty: Some(Type::number()),
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
        assert_eq!(resolved.unwrap().to_string(), "utils::format");
    }

    #[test]
    fn test_resolve_use_path_self() {
        let registry = ModuleRegistry::new();
        let from = ModulePath::from_str_segments(&["utils", "main"]).unwrap();
        let path = vec![Arc::from("sibling")];

        let resolved = registry.resolve_use_path(&from, &UsePrefix::Self_, &path);
        assert!(resolved.is_ok());
        assert_eq!(resolved.unwrap().to_string(), "utils::sibling");
    }

    #[test]
    fn test_resolve_use_path_super() {
        let registry = ModuleRegistry::new();
        let from = ModulePath::from_str_segments(&["a", "b", "c"]).unwrap();
        let path = vec![Arc::from("other")];

        let resolved = registry.resolve_use_path(&from, &UsePrefix::Super(1), &path);
        assert!(resolved.is_ok());
        assert_eq!(resolved.unwrap().to_string(), "a::other");
    }

    #[test]
    fn test_resolve_use_path_core() {
        let registry = ModuleRegistry::new();
        let from = ModulePath::from_str_segments(&["main"]).unwrap();
        let path = vec![Arc::from("List")];

        let resolved = registry
            .resolve_use_path(&from, &UsePrefix::Core, &path)
            .expect("core resolves under the reserved root");
        assert_eq!(resolved.to_string(), "core::List");
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
                    path: vec![
                        (Arc::from("utils"), Span::default()),
                        (Arc::from("helper"), Span::default()),
                    ],
                    alias: None,
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

    /// A `pub use pkg::donor::gift;` item, for building a prelude module.
    fn pub_use(segments: &[&str]) -> crate::ast::Item {
        use crate::ast::{Item, UseDef};
        Item::new(
            ItemKind::Use(UseDef {
                is_public: true,
                prefix: UsePrefix::Pkg,
                path: segments
                    .iter()
                    .map(|s| (Arc::from(*s), Span::default()))
                    .collect(),
                alias: None,
            }),
            Span::default(),
        )
    }

    #[test]
    fn prelude_binds_at_lowest_precedence_and_imports_shadow_it() {
        // `donor` defines `gift`; `pre` re-exports it; `pre` is the prelude.
        let mut registry = ModuleRegistry::new();
        let donor = ModulePath::from_str_segments(&["donor"]).unwrap();
        registry.register(
            &donor,
            Arc::new(Module {
                name: Arc::from("donor"),
                doc: None,
                items: vec![make_function("gift", true)],
            }),
        );
        let other = ModulePath::from_str_segments(&["other"]).unwrap();
        registry.register(
            &other,
            Arc::new(Module {
                name: Arc::from("other"),
                doc: None,
                items: vec![make_function("gift", true)],
            }),
        );
        let pre = ModulePath::from_str_segments(&["pre"]).unwrap();
        registry.register(
            &pre,
            Arc::new(Module {
                name: Arc::from("pre"),
                doc: None,
                items: vec![pub_use(&["donor", "gift"])],
            }),
        );
        registry.set_prelude(pre.clone());

        // A consumer with no `use` sees `gift` only through the prelude tier —
        // never in `items` (which the dep-closure loop turns into edges).
        let consumer = ModulePath::from_str_segments(&["consumer"]).unwrap();
        registry.register(
            &consumer,
            Arc::new(Module {
                name: Arc::from("consumer"),
                doc: None,
                items: vec![],
            }),
        );
        let scope = registry.build_module_scope(&consumer);
        assert!(
            scope.items.get("gift").is_none(),
            "prelude names must not live in `items`"
        );
        let bound = scope
            .item("gift", Namespace::Value)
            .expect("prelude `gift` resolves");
        assert_eq!(bound.module, donor);

        // An explicit `use pkg::other::gift;` shadows the prelude at the same
        // name: `items` wins over `prelude_items`.
        let shadower = ModulePath::from_str_segments(&["shadower"]).unwrap();
        registry.register(
            &shadower,
            Arc::new(Module {
                name: Arc::from("shadower"),
                doc: None,
                items: vec![{
                    use crate::ast::{Item, UseDef};
                    Item::new(
                        ItemKind::Use(UseDef {
                            is_public: false,
                            prefix: UsePrefix::Pkg,
                            path: vec![
                                (Arc::from("other"), Span::default()),
                                (Arc::from("gift"), Span::default()),
                            ],
                            alias: None,
                        }),
                        Span::default(),
                    )
                }],
            }),
        );
        let scope = registry.build_module_scope(&shadower);
        let bound = scope
            .item("gift", Namespace::Value)
            .expect("explicit `gift` resolves");
        assert_eq!(bound.module, other, "an explicit `use` shadows the prelude");
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
                    alias: None,
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
                alias: None,
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
                items: use_items(UsePrefix::Pkg, &["a"], &["b"], false),
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

    /// The old braced form flattens to one `UseDef` per item at lowering;
    /// tests build the flattened items directly.
    fn use_items(prefix: UsePrefix, path: &[&str], items: &[&str], is_public: bool) -> Vec<Item> {
        items
            .iter()
            .map(|item| {
                let mut full: Vec<&str> = path.to_vec();
                full.push(item);
                use_module(prefix, &full, is_public)
            })
            .collect()
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
                items: use_items(UsePrefix::Pkg, &["origin"], &["helper"], true),
            }),
        );

        let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
        registry.register(
            &main_path,
            Arc::new(Module {
                name: Arc::from("main"),
                doc: None,
                items: use_items(UsePrefix::Pkg, &["facade"], &["helper"], false),
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
                items: [
                    use_items(UsePrefix::Pkg, &["utils"], &["missing"], false),
                    use_items(UsePrefix::Pkg, &["utils"], &["secret"], false),
                    use_items(UsePrefix::Pkg, &["nonexistent"], &["anything"], false),
                ]
                .concat(),
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

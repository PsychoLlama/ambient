//! Module registry for cross-module name resolution.
//!
//! The module registry tracks all loaded modules and their exported symbols,
//! enabling cross-module name resolution during type checking and compilation.
//!
//! Layout:
//! - [`mod@self`]: [`ModuleRegistry`] itself — registration, module/symbol
//!   lookup, and path resolution — plus [`RegistryError`] and [`ModuleInfo`].
//! - [`exports`]: [`ExportInfo`]/[`ExportKind`]/[`ReExport`] and the
//!   extraction of a module's exports from its AST.
//! - [`scope`]: [`ModuleScope`]/[`ItemImport`]/[`Namespace`] and the
//!   `use`-interpretation + prelude-injection impl block.
//! - [`imports`]: [`ResolvedImports`], the flat binding view over a scope.

mod exports;
mod imports;
mod scope;
#[cfg(test)]
mod tests;

pub use exports::{ExportInfo, ExportKind, ReExport};
pub use imports::{ImportError, ResolvedImport, ResolvedImports};
pub use scope::{ItemImport, ModuleScope, Namespace};

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use crate::ast::{ItemKind, Module, UsePrefix};
use crate::fqn::{Fqn, ModuleId};
use crate::module_path::{ImportPrefix, ModulePath, ResolutionError};

use exports::{extract_exports, extract_re_exports};

/// The foreign-ability channel every registry-backed compile needs: each
/// registered module's `ability` declarations resolved to their uuid-derived
/// identities and method keys, keyed by [`Fqn`]. Wrapped in an `Arc` so the
/// registry's memoized table (see [`ModuleRegistry::foreign_abilities`]) is
/// shared, not re-cloned, into every
/// [`ModuleEnv`](crate::module_env::ModuleEnv).
pub type ForeignAbilityTable = Arc<Vec<(Fqn, Arc<crate::ability_resolver::DynAbility>)>>;

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

/// Registry of all loaded modules.
///
/// The registry maintains a map from module paths to their exports,
/// enabling cross-module name resolution. Cloning is cheap relative to
/// building: module ASTs are shared through `Arc`.
#[derive(Debug)]
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
    /// Host-provided implementations for `extern fn` declarations. The
    /// engine registers `core`'s bindings in `register_core_modules`;
    /// embedders add their own via [`Self::register_natives`]. Compiles
    /// read it through [`crate::module_env::ModuleEnv`].
    natives: crate::natives::NativeRegistry,
    /// Bumped on every mutation that can change the foreign-ability table —
    /// module registration, the workspace name, the prelude, injected
    /// exports. Guards [`Self::ability_cache`] so a stale memo can never be
    /// served: the reader recomputes whenever the revision has moved.
    ability_revision: u64,
    /// Memoized [`Self::foreign_abilities`] table, tagged with the
    /// [`Self::ability_revision`] it was computed at. Resolving abilities is
    /// deterministic but O(modules), and every registry-backed compile needs
    /// the whole table, so a per-module `ModuleEnv::new` would otherwise make
    /// the build O(modules²). Interior-mutable so the read path stays
    /// `&self`; each registry owns its cache (a clone starts empty — see the
    /// manual [`Clone`] impl — so a diverging clone never reads a sibling's
    /// table).
    ability_cache: Mutex<Option<(u64, ForeignAbilityTable)>>,
}

impl Clone for ModuleRegistry {
    fn clone(&self) -> Self {
        Self {
            modules: self.modules.clone(),
            workspace_name: Arc::clone(&self.workspace_name),
            prelude: self.prelude.clone(),
            natives: self.natives.clone(),
            ability_revision: self.ability_revision,
            // A fresh, empty cache — never alias the source registry's memo,
            // so a clone that later diverges can't read a stale table keyed
            // to a revision that means something different here.
            ability_cache: Mutex::new(None),
        }
    }
}

impl Default for ModuleRegistry {
    fn default() -> Self {
        Self {
            modules: HashMap::new(),
            workspace_name: Arc::from(""),
            prelude: None,
            natives: crate::natives::NativeRegistry::new(),
            ability_revision: 0,
            ability_cache: Mutex::new(None),
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
        // Item `Fqn`s are scoped under this name, so every cached ability
        // identity's key is now stale.
        self.ability_revision += 1;
    }

    /// The host's native bindings for `extern fn` declarations.
    #[must_use]
    pub fn natives(&self) -> &crate::natives::NativeRegistry {
        &self.natives
    }

    /// The foreign-ability table for a registry-backed compile: every
    /// registered module's `ability` declarations resolved to their
    /// uuid-derived identities and method keys, keyed by [`Fqn`].
    ///
    /// Memoized against [`Self::ability_revision`]: resolution is
    /// deterministic and O(modules), and every module's `ModuleEnv` needs the
    /// full table, so recomputing per compile would make a build O(modules²).
    /// The cache is revision-guarded, so any mutation that could change the
    /// table forces a recompute rather than serving stale identities.
    #[must_use]
    pub fn foreign_abilities(&self) -> ForeignAbilityTable {
        let mut cache = self
            .ability_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some((rev, table)) = cache.as_ref()
            && *rev == self.ability_revision
        {
            return Arc::clone(table);
        }
        let table: ForeignAbilityTable = Arc::new(crate::infer::resolve_registry_abilities(self));
        *cache = Some((self.ability_revision, Arc::clone(&table)));
        table
    }

    /// Register native bindings (mutable access for the host wiring phase:
    /// engine core bindings, then any embedder bindings).
    pub fn natives_mut(&mut self) -> &mut crate::natives::NativeRegistry {
        &mut self.natives
    }

    /// Verify the extern-fn contract across every registered module: each
    /// declaration bound (with matching arity), each binding backed by a
    /// declaration. Call after all modules and bindings are registered;
    /// violations are build errors.
    #[must_use]
    pub fn verify_native_contract(&self) -> Vec<crate::natives::ContractViolation> {
        self.natives.verify_contract(
            self.modules
                .values()
                .map(|info| (&info.path, info.module.as_ref())),
        )
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
        // Prelude injection feeds import resolution, which resolves the type
        // names in ability method signatures — a signature change re-keys
        // methods, so invalidate.
        self.ability_revision += 1;
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
        // A newly registered (or replaced) module may add, change, or drop
        // `ability` declarations.
        self.ability_revision += 1;
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
    /// item of its module — `use core::primitives::number::sqrt;` must resolve exactly
    /// like a compiled function would.
    pub fn add_exports(&mut self, path: &ModulePath, exports: Vec<ExportInfo>) {
        // Injected exports feed import resolution (the surface a signature's
        // type names resolve against), so invalidate the ability memo to stay
        // correctness-first even though today's intrinsics are functions.
        self.ability_revision += 1;
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
}

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
#[cfg(test)]
mod mount_tests;
mod scope;
#[cfg(test)]
mod tests;

pub use exports::{ExportInfo, ExportKind, ReExport};
pub use imports::{ImportError, ResolvedImport, ResolvedImports};
pub use scope::{ItemImport, ModuleScope, Namespace};

use std::collections::{BTreeMap, HashMap};
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

    /// An ability-method path (`use m::Ability::method;`) named an
    /// ability that exists but declares no such method.
    #[error("ability `{ability}` has no method `{method}`")]
    AbilityMethodNotFound { ability: String, method: String },

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
    /// The module's real on-disk source path, relative to the package `src/`
    /// directory (`collections/main.ab`), when the loader knows it. `None`
    /// for embedded builtins (no on-disk file) and for callers that register
    /// without a path — then consumers reconstruct it from the module path
    /// (see [`crate::module_interface::module_source_path`]). Recording the
    /// real path is what lets a directory module resolve to `<dir>/main.ab`
    /// instead of the reconstructed `<dir>.ab`.
    pub source_path: Option<String>,
}

/// Registry of all loaded modules.
///
/// The registry maintains a map from module paths to their exports,
/// enabling cross-module name resolution. Cloning is cheap relative to
/// building: module ASTs are shared through `Arc`.
#[derive(Debug)]
pub struct ModuleRegistry {
    /// Map from module path string to module info. A `BTreeMap` (not a
    /// `HashMap`) so [`Self::all_modules`] iterates in a deterministic,
    /// registration-order-independent order (ascending by module-path string) —
    /// build determinism (warm == cold == lazy byte-identity) and stable
    /// diagnostic/completion ordering then hold by construction rather than
    /// relying on every downstream consumer to launder a nondeterministic
    /// iteration order through its own sort.
    modules: BTreeMap<String, ModuleInfo>,
    /// The workspace package name (`ambient.toml` `name`) user modules are
    /// scoped under (`workspace::<name>`). Empty until
    /// [`Self::set_workspace_name`] runs — a consistent placeholder that
    /// keeps every key internally consistent within one build. Only
    /// consulted for modules that are not under a package mount (see
    /// [`Self::mounts`]): the registry-less/bare layout single-file checks
    /// and the REPL use.
    workspace_name: Arc<str>,
    /// Package mounts: the package names whose modules are registered under
    /// a leading name segment (`["foo", "utils"]` for package `foo`'s
    /// `src/utils.ab`; the package root `main.ab` collapses to the mount
    /// itself, `["foo"]`, as a directory module). Mounting is what lets
    /// several packages share one registry without key collisions —
    /// [`Self::module_id`] strips a mount into
    /// [`Scope::Workspace`](crate::fqn::Scope) exactly like a leading
    /// `core` strips into `Builtin`. Manifest-backed builds always mount
    /// (a single package mounts alone); only registry-less bare layouts
    /// leave this empty.
    mounts: std::collections::BTreeSet<Arc<str>>,
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
            mounts: self.mounts.clone(),
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
            modules: BTreeMap::new(),
            workspace_name: Arc::from(""),
            mounts: std::collections::BTreeSet::new(),
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

    /// Mount a package: its modules are registered under a leading `name`
    /// segment, and [`Self::module_id`] scopes them under
    /// `workspace::<name>`. Call once per package before registering its
    /// modules.
    pub fn add_mount(&mut self, name: impl Into<Arc<str>>) {
        self.mounts.insert(name.into());
        // Mounting changes how every mounted module's `Fqn` is minted.
        self.ability_revision += 1;
    }

    /// The mounted package names, in order.
    pub fn mounts(&self) -> impl Iterator<Item = &Arc<str>> {
        self.mounts.iter()
    }

    /// The package mount `path` lives under: its leading segment when that
    /// names a mounted package, else `None` (a bare-layout module, or a
    /// reserved `core` path).
    #[must_use]
    pub fn mount_of(&self, path: &ModulePath) -> Option<&Arc<str>> {
        let first = path.segments().first()?;
        self.mounts.get(first)
    }

    /// The leading segments `pkg::` resolves under (and `super` may not
    /// escape) for a module at `path`: its package mount, or empty for the
    /// bare layout.
    #[must_use]
    pub fn package_root_of(&self, path: &ModulePath) -> Vec<Arc<str>> {
        self.mount_of(path)
            .map_or_else(Vec::new, |mount| vec![Arc::clone(mount)])
    }

    /// The package name a module at `path` belongs to: its mount, or the
    /// bare-layout workspace name.
    #[must_use]
    pub fn package_name_of(&self, path: &ModulePath) -> Arc<str> {
        self.mount_of(path)
            .map_or_else(|| Arc::clone(&self.workspace_name), Arc::clone)
    }

    /// The [`ModuleId`] for a module path under this registry's workspace.
    ///
    /// A path under a package mount scopes to that package with the mount
    /// segment stripped (`["foo", "utils"]` → `workspace::foo::utils`; the
    /// mount itself, `["foo"]`, is the package's root module,
    /// `workspace::foo`). A leading `core` folds into `Builtin`; anything
    /// else is bare-layout and scopes under [`Self::workspace_name`].
    #[must_use]
    pub fn module_id(&self, path: &ModulePath) -> ModuleId {
        if let Some(mount) = self.mount_of(path) {
            return ModuleId {
                scope: crate::fqn::Scope::Workspace(Arc::clone(mount)),
                path: path.segments()[1..].to_vec(),
            };
        }
        ModuleId::from_module_path(path, &self.workspace_name)
    }

    /// The [`ModulePath`] a [`ModuleId`] is registered under — the inverse
    /// of [`Self::module_id`]. Re-attaches the mount segment for a mounted
    /// package and the reserved `core` segment for a builtin. `None` only
    /// for the degenerate empty bare-layout module.
    #[must_use]
    pub fn module_path_of(&self, id: &ModuleId) -> Option<ModulePath> {
        if let crate::fqn::Scope::Workspace(pkg) = &id.scope
            && self.mounts.contains(pkg)
        {
            let mut segments = vec![Arc::clone(pkg)];
            segments.extend(id.path.iter().cloned());
            return ModulePath::from_segments(segments);
        }
        id.to_module_path()
    }

    /// The dotted module-path key for a [`ModuleId`] (`foo::utils`,
    /// `core::primitives`) — the mount-aware replacement for
    /// [`ModuleId::module_path_string`], matching this registry's actual
    /// registration keys.
    #[must_use]
    pub fn module_key(&self, id: &ModuleId) -> String {
        self.module_path_of(id)
            .map_or_else(String::new, |path| path.to_string())
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

        // Preserve a previously recorded on-disk source path across a
        // re-registration (raw → resolved AST, or an editor edit): the path is
        // a filesystem fact that a resolve pass never changes.
        let source_path = self
            .modules
            .get(&path.to_string())
            .and_then(|prev| prev.source_path.clone());
        let info = ModuleInfo {
            path: path.clone(),
            module,
            exports,
            re_exports,
            is_namespace: false,
            is_dir_module,
            source_path,
        };

        self.modules.insert(path.to_string(), info);
        self.register_namespace_ancestors(path);
        // A newly registered (or replaced) module may add, change, or drop
        // `ability` declarations.
        self.ability_revision += 1;
    }

    /// Record a module's real on-disk source path (relative to the package
    /// `src/` directory). The loader knows the actual file — including whether
    /// a directory module lives at `<dir>/main.ab` — so recording it here lets
    /// [`crate::module_interface::module_source_path`] serve the true path
    /// rather than reconstructing one from the module path. A no-op for an
    /// unregistered path.
    pub fn set_source_path(&mut self, path: &ModulePath, source_path: String) {
        if let Some(info) = self.modules.get_mut(&path.to_string()) {
            info.source_path = Some(source_path);
        }
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
                    // A pure namespace has no backing file of its own.
                    source_path: None,
                },
            );
            ancestor = dir.parent();
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

    /// The nominal `unique(<uuid>)` identity of the ability named `name` in
    /// `module`, if it declares one. The uuid is an ability's content identity
    /// (kept off the [`Fqn`] on purpose), so a consumer holding a *resolved*
    /// ability reference — module plus name — looks it up here rather than
    /// re-scanning the defining module itself.
    #[must_use]
    pub fn ability_uuid(&self, module: &ModulePath, name: &str) -> Option<uuid::Uuid> {
        self.get(module)?
            .module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Ability(a) if a.name.as_ref() == name => Some(a.uuid),
                _ => None,
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
    ) -> Result<(ExportInfo, ModulePath), RegistryError> {
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
            return Ok((export.clone(), module_path.clone()));
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
            // A method re-export (`pub use m::Ability::method;`): the
            // re-export path's parent segment names an ability, not a
            // module. Chase it through the same ability-method lookup a
            // direct `use` takes.
            if let Some((ability, grandparent)) = parent.split_last()
                && let Some(gp_path) = self.resolve_import_path(module_path, re_export, grandparent)
                && let Ok(resolved) = self.lookup_ability_method(&gp_path, ability, original)
            {
                return Ok(resolved);
            }
        }

        Err(RegistryError::SymbolNotFound {
            module: module_path.to_string(),
            symbol: symbol_name.to_string(),
        })
    }

    /// Look up one method of an ability as an importable symbol:
    /// `ability_name` is resolved as an exported symbol of `module_path`
    /// (chasing `pub use` re-export chains and enforcing visibility,
    /// exactly like [`Self::lookup_symbol`]), and `method_name` must be a
    /// method its defining declaration carries. Methods are never
    /// module-level exports — this synthesized [`ExportKind::AbilityMethod`]
    /// export (its `owner` naming the ability) is reachable only through
    /// the explicit `use m::Ability::method;` path shape and method
    /// re-exports.
    ///
    /// # Errors
    ///
    /// Everything [`Self::lookup_symbol`] reports for the ability segment
    /// (missing module, missing symbol, not public); `SymbolNotFound` when
    /// the parent segment resolves to a non-ability;
    /// [`RegistryError::AbilityMethodNotFound`] when the ability exists
    /// but declares no such method.
    pub fn lookup_ability_method(
        &self,
        module_path: &ModulePath,
        ability_name: &str,
        method_name: &str,
    ) -> Result<(ExportInfo, ModulePath), RegistryError> {
        let (ability_export, origin) = self.lookup_symbol(module_path, ability_name)?;
        if ability_export.kind != ExportKind::Ability {
            return Err(RegistryError::SymbolNotFound {
                module: module_path.to_string(),
                symbol: format!("{ability_name}::{method_name}"),
            });
        }
        let def = self
            .get(&origin)
            .and_then(|info| {
                info.module.items.iter().find_map(|item| match &item.kind {
                    ItemKind::Ability(def) if def.name == ability_export.name => Some(def),
                    _ => None,
                })
            })
            .ok_or_else(|| RegistryError::ModuleNotFound(origin.to_string()))?;
        let method = def
            .methods
            .iter()
            .find(|m| m.name.as_ref() == method_name)
            .ok_or_else(|| RegistryError::AbilityMethodNotFound {
                ability: ability_export.name.to_string(),
                method: method_name.to_string(),
            })?;
        Ok((
            ExportInfo {
                name: Arc::clone(&method.name),
                kind: ExportKind::AbilityMethod,
                is_public: ability_export.is_public,
                re_export_from: None,
                name_span: method.name_span,
                doc: None,
                owner: Some(Arc::clone(&def.name)),
            },
            origin,
        ))
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
            UsePrefix::Workspace => ImportPrefix::Workspace,
            UsePrefix::Core => ImportPrefix::Core,
            UsePrefix::Self_ => ImportPrefix::Self_,
            UsePrefix::Super(count) => ImportPrefix::Super(count),
            // Alias-rooted re-exports are rejected at scope building; a
            // stray one resolves to nothing.
            UsePrefix::Local => return None,
        };

        from.resolve_relative(
            &prefix,
            path,
            self.is_dir_module(from),
            &self.package_root_of(from),
        )
        .ok()
    }

    /// Get all registered modules, in ascending module-path-string order.
    ///
    /// The order is deterministic and independent of registration order (the
    /// backing store is a `BTreeMap`). Downstream consumers may rely on this
    /// for build determinism and stable diagnostic/completion ordering, and
    /// need not re-sort to launder a nondeterministic iteration order — though
    /// a consumer that wants a *different* order (e.g. a declaration module
    /// hoisted first, or a canonical multiset hash over shape bytes) must still
    /// impose it.
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
            UsePrefix::Workspace => ImportPrefix::Workspace,
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

        from.resolve_relative(
            &import_prefix,
            path,
            self.is_dir_module(from),
            &self.package_root_of(from),
        )
        .map_err(RegistryError::PathResolution)
    }
}

//! Module import scopes: interpreting `use` items into canonical bindings,
//! plus prelude injection.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{ItemKind, Span, UseDef, UsePrefix};
use crate::fqn::{Fqn, ModuleId};
use crate::module_path::ModulePath;

use super::{ExportInfo, ExportKind, ImportError, ModuleRegistry, RegistryError};

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
    /// For an [`ExportKind::AbilityMethod`] binding, the name of the
    /// ability (in the defining module) that declares the method. `None`
    /// for every other kind.
    pub owner: Option<Arc<str>>,
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
    /// Imported ability methods. Their own namespace, not `Value`: a
    /// perform site is syntactically distinct (`seed!(…)`), so a method
    /// import never collides with a same-named function or const.
    AbilityMethod,
    Trait,
}

impl ExportKind {
    /// The namespace this kind of item occupies.
    #[must_use]
    pub fn namespace(self) -> Namespace {
        match self {
            Self::Function | Self::Const | Self::EnumVariant => Namespace::Value,
            Self::Struct | Self::TypeAlias | Self::Enum => Namespace::Type,
            Self::Ability | Self::Set => Namespace::Ability,
            Self::AbilityMethod => Namespace::AbilityMethod,
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

impl ModuleRegistry {
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
                owner: export.owner.clone(),
                span: Span::default(),
            };
            let entry = scope.prelude_items.entry(Arc::from(local)).or_default();
            entry.retain(|existing| existing.kind.namespace() != ns);
            entry.push(import);
        }
    }

    /// The `extern` struct declarations the prelude contributes to every
    /// module: for each public prelude re-export whose origin is an `extern`
    /// unit struct, the exported name mapped to that struct's declaration.
    ///
    /// This is the module-system source of the four primitive nominals
    /// (`Bool`/`Number`/`String`/`Binary`) and the opaque generic containers
    /// (`List`/`Map`/`Set`). Rather than hardcode `Type::string()` and
    /// friends, they are discovered by walking `core::prelude`'s re-exports
    /// exactly the way [`Self::inject_prelude`] resolves them — through
    /// [`Self::lookup_symbol`], chasing `pub use` chains to the defining
    /// `extern` declaration. Enums (`Option`/`Result`) and non-re-exported
    /// types (`Duration`) are naturally excluded: only a re-exported `extern`
    /// unit struct qualifies. The whole declaration is returned (not just its
    /// type) so the consumer registers it by the same rule as every other
    /// channel (`AliasTarget::of_struct`), type parameters included.
    ///
    /// Ability resolution seeds these so a primitive or container named in an
    /// ability signature (`Stdio.out(String)`, `Env.args(): List<String>`)
    /// resolves to its uuid-carrying form — keeping ability content hashes
    /// byte-stable — without the checker carrying a context-independent
    /// `Primitive::from_name`/`Container::from_name` shortcut.
    #[must_use]
    pub fn prelude_struct_defs(&self) -> Vec<(Arc<str>, crate::ast::StructDef)> {
        let mut defs = Vec::new();
        let Some(prelude_path) = &self.prelude else {
            return defs;
        };
        let Some(prelude_info) = self.modules.get(&prelude_path.to_string()) else {
            return defs;
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
                    defs.push((Arc::from(local), def.clone()));
                }
            }
        }
        defs
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
                        owner: export.owner.clone(),
                        span,
                    },
                );
            }
            Err(error) => {
                // `use m::Ability::method;` — the parent segment may name
                // an ability rather than a module. Bind the method when it
                // does; report whichever error is more specific otherwise.
                match self.use_target_as_ability_method(&symbol_parent, original) {
                    Some(Ok((export, origin))) => {
                        scope.bind_item(
                            local,
                            ItemImport {
                                module: origin,
                                name: Arc::clone(&export.name),
                                kind: export.kind,
                                owner: export.owner.clone(),
                                span,
                            },
                        );
                    }
                    Some(Err(method_error)) if submodule.is_none() => {
                        scope.errors.push(ImportError {
                            error: method_error,
                            span,
                        });
                    }
                    // Only a diagnostic if no namespace bound the name; a
                    // successful submodule import means the missing symbol
                    // (or missing parent module) was never what the user
                    // meant.
                    None if submodule.is_none() => {
                        scope.errors.push(ImportError { error, span });
                    }
                    _ => {}
                }
            }
        }
    }

    /// Interpret a failed `use` leaf as an ability-method import: the
    /// leaf's parent segment (`m::Ability` in `use m::Ability::method;`)
    /// names an ability exported by *its* parent module. Returns `None`
    /// when the shape doesn't apply (no ability of that name there), so
    /// the caller reports the original module/symbol error; `Some(Err(…))`
    /// when the ability exists but the method lookup fails (missing
    /// method, private ability) — the more specific diagnostic.
    fn use_target_as_ability_method(
        &self,
        symbol_parent: &ModulePath,
        method: &str,
    ) -> Option<Result<(ExportInfo, ModulePath), RegistryError>> {
        if symbol_parent.segments().is_empty() {
            return None;
        }
        let ability = symbol_parent.name().to_string();
        let ability_parent = symbol_parent.parent().unwrap_or_else(ModulePath::root);
        match self.lookup_ability_method(&ability_parent, &ability, method) {
            Ok(found) => Some(Ok(found)),
            // The ability exists but the method doesn't, or the ability is
            // private: the path shape matched, so this is the right error.
            Err(
                error @ (RegistryError::AbilityMethodNotFound { .. }
                | RegistryError::NotPublic { .. }),
            ) => Some(Err(error)),
            Err(_) => None,
        }
    }
}

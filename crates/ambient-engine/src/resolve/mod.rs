//! The resolve pass: canonicalize every resolved reference to its `Fqn`.
//!
//! Every item in a build has exactly one fully-qualified identity —
//! `<defining module>::<item name>` — and this pass maps each *spelling*
//! of a reference to that identity, whether it names an item in another
//! module or in the current one:
//!
//! - a bare same-module name (a sibling `fn`/`const`/variant/type/ability),
//! - a bare imported name (`double` after `use pkg::util::double;`),
//! - a module-alias path (`util::double`, `nested::leaf::leaf_fn`),
//! - an inline rooted path (`pkg::util::double`, `core::primitives::number::sqrt`,
//!   `self::sibling::helper`, `core::system::Stdio`).
//!
//! Only true lexical **locals** (params, `let`, pattern/lambda bindings)
//! stay bare — they are not items. An enum variant resolves to the
//! two-segment ident `Fqn(module, [Enum, Variant])`, both same-module and
//! imported.
//!
//! The canonical identity is recorded in [`QualifiedName::resolved`]
//! without disturbing the source spelling (whose spans serve IDE
//! features). Downstream consumers — the type checker's environment, the
//! intrinsic tables, the ability resolver, and the compiler's linking
//! table — key strictly off [`QualifiedName::resolution_key`], so the
//! rule "anything reachable fully-qualified works through `use`, and
//! vice versa" holds by construction: both spellings resolve to the same
//! key.
//!
//! The pass never *reports* use-site errors: a reference it cannot
//! resolve is left untouched, and the type checker produces the
//! diagnostic (undefined variable / unknown ability) exactly as it would
//! have before. Import errors on `use` items themselves are reported by
//! the checker from [`ModuleRegistry::build_module_scope`].

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::ast::{ItemKind, Module};
use crate::fqn::{Fqn, ModuleId};
use crate::module_path::ModulePath;
use crate::module_registry::{ImportError, ItemImport, ModuleRegistry, ModuleScope, Namespace};

/// Resolve every cross-module reference in `module` to its canonical
/// identity. See the module docs for the contract.
///
/// Returns the set of foreign modules the module's references resolved
/// into (dotted paths, ordered) — the module's true dependency set, which
/// the build uses for compilation ordering. Idempotent: already-resolved
/// references are left alone but still counted.
pub fn resolve_module(
    module: &mut Module,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
) -> ResolveOutcome {
    let scope = registry.build_module_scope(module_path);
    let mut resolver = Resolver::new(module, module_path, registry, scope);
    resolver.resolve(module);
    // Imports themselves are dependencies even when unreferenced: their
    // targets must exist (and their enums/abilities register) for the
    // module to check.
    for import in resolver.scope.items.values().flatten() {
        if &import.module != module_path {
            let id = registry.module_id(&import.module);
            resolver.deps.insert(id);
        }
    }
    ResolveOutcome {
        deps: resolver.deps,
        errors: resolver.import_errors,
    }
}

/// What the resolve pass learned about a module.
pub struct ResolveOutcome {
    /// Foreign modules the module's references resolved into (as
    /// [`ModuleId`]s, ordered): its true dependency set, which the build
    /// uses for compilation ordering.
    pub deps: BTreeSet<ModuleId>,
    /// Failed block-scoped imports. (Module-level import failures are
    /// reported by the checker from the module scope; block-level `use`
    /// items only exist here.)
    pub errors: Vec<ImportError>,
}

struct Resolver<'r> {
    registry: &'r ModuleRegistry,
    current: &'r ModulePath,
    /// Whether `current` is a directory module (backed by a `main.ab`),
    /// which anchors inline `self::`/`super::` at its own path.
    current_is_dir: bool,
    scope: ModuleScope,
    /// Module-level value names (functions, consts, unit-struct values):
    /// these shadow imports for bare references, and resolve to their
    /// own `Fqn(current, [name])`. Enum variants live in
    /// [`Self::module_variants`] instead (they carry a two-segment ident).
    module_values: HashSet<Arc<str>>,
    /// Module-level enum variant names → their declaring enum's name. A
    /// same-module variant reference resolves to `Fqn(current, [Enum,
    /// Variant])`; the map supplies the `Enum` segment.
    module_variants: HashMap<Arc<str>, Arc<str>>,
    /// Module-level type-namespace names (type aliases, enums): a path
    /// head naming one of these is a type-associated call
    /// (`Money::default()`), which the checker resolves.
    module_types: HashSet<Arc<str>>,
    /// Module-level ability names: bare ability references to these stay
    /// bare (the ability resolver registers local declarations by name).
    module_abilities: HashSet<Arc<str>>,
    /// Lexical scope stack of local binding names (params, lets, pattern
    /// bindings, handler params). Locals shadow everything.
    locals: Vec<Vec<Arc<str>>>,
    /// Import overlays from block-scoped `use` statements, innermost
    /// last. Consulted before the module scope; popped with their block.
    overlays: Vec<ModuleScope>,
    /// Failed block-scoped imports, surfaced through [`ResolveOutcome`].
    import_errors: Vec<ImportError>,
    /// Foreign modules that references resolved into (as [`ModuleId`]s).
    deps: BTreeSet<ModuleId>,
}

impl<'r> Resolver<'r> {
    fn new(
        module: &Module,
        current: &'r ModulePath,
        registry: &'r ModuleRegistry,
        scope: ModuleScope,
    ) -> Self {
        let mut module_values = HashSet::new();
        let mut module_variants = HashMap::new();
        let mut module_types = HashSet::new();
        let mut module_abilities = HashSet::new();
        for item in &module.items {
            match &item.kind {
                ItemKind::Function(f) => {
                    module_values.insert(Arc::clone(&f.name));
                }
                ItemKind::ExternFn(e) => {
                    module_values.insert(Arc::clone(&e.name));
                }
                ItemKind::Const(c) => {
                    module_values.insert(Arc::clone(&c.name));
                }
                ItemKind::Struct(s) => {
                    module_types.insert(Arc::clone(&s.name));
                    // A unit struct is a value too — its bare name constructs
                    // it — so it stays bare (unresolved) like an enum variant,
                    // and the checker's bare-name value binding covers it. An
                    // `extern` unit struct is a type only (engine-provided), so
                    // it does not bind a value.
                    if s.is_unit_value() {
                        module_values.insert(Arc::clone(&s.name));
                    }
                }
                ItemKind::TypeAlias(t) => {
                    module_types.insert(Arc::clone(&t.name));
                }
                ItemKind::Enum(e) => {
                    module_types.insert(Arc::clone(&e.name));
                    for variant in &e.variants {
                        module_variants.insert(Arc::clone(&variant.name), Arc::clone(&e.name));
                    }
                }
                ItemKind::Ability(a) => {
                    module_abilities.insert(Arc::clone(&a.name));
                }
                ItemKind::Trait(_) | ItemKind::Impl(_) | ItemKind::Use(_) => {}
            }
        }
        let current_is_dir = registry.get(current).is_some_and(|info| info.is_dir_module);
        Self {
            registry,
            current,
            current_is_dir,
            scope,
            module_values,
            module_variants,
            module_types,
            module_abilities,
            locals: Vec::new(),
            overlays: Vec::new(),
            import_errors: Vec::new(),
            deps: BTreeSet::new(),
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // Scopes
    // ─────────────────────────────────────────────────────────────────────

    /// The item bound to `name` in `ns`, innermost block overlay first.
    fn scope_item(&self, name: &str, ns: Namespace) -> Option<&ItemImport> {
        self.overlays
            .iter()
            .rev()
            .find_map(|overlay| overlay.item(name, ns))
            .or_else(|| self.scope.item(name, ns))
    }

    /// The module alias bound to `name`, innermost block overlay first.
    fn scope_module(&self, name: &str) -> Option<&ModulePath> {
        self.overlays
            .iter()
            .rev()
            .find_map(|overlay| overlay.module(name))
            .or_else(|| self.scope.module(name))
    }

    fn is_local(&self, name: &str) -> bool {
        self.locals
            .iter()
            .any(|frame| frame.iter().any(|n| n.as_ref() == name))
    }

    // ─────────────────────────────────────────────────────────────────────
    // Reference resolution
    // ─────────────────────────────────────────────────────────────────────

    /// Build the [`Fqn`] identity for `ident` defined in `module`,
    /// recording a foreign module as a dependency. Same-module references
    /// resolve to their *full* `Fqn` (current module + ident) — there is no
    /// bare special case.
    fn canonical(&mut self, module: &ModulePath, ident: Vec<Arc<str>>) -> Fqn {
        let module_id = self.registry.module_id(module);
        if module != self.current {
            self.deps.insert(module_id.clone());
        }
        Fqn::new(module_id, ident)
    }

    /// Whether `module` exports `name` (publicly), or `module` is the
    /// current module and declares it at all.
    fn item_exists(&self, module: &ModulePath, name: &str) -> bool {
        if module == self.current {
            return self.module_values.contains(name)
                || self.module_variants.contains_key(name)
                || self.module_types.contains(name)
                || self.module_abilities.contains(name);
        }
        self.registry.lookup_symbol(module, name).is_ok()
    }

    /// Resolve a reference's path segments to the module they name.
    ///
    /// The head segment may be a root keyword (`pkg`, `core`, `self`,
    /// `super`) or a module alias from a `use` item; every
    /// following segment must name a child module (a submodule file, a
    /// directory namespace, or a module re-export). Returns `None` — and
    /// leaves the reference for the checker to diagnose — when the path
    /// doesn't lead through modules (e.g. `Money::default()`, where the
    /// head is a type).
    fn resolve_module_prefix(&self, path: &[Arc<str>]) -> Option<ModulePath> {
        let head = path.first()?;
        let (mut cursor, rest): (Option<ModulePath>, &[Arc<str>]) = match head.as_ref() {
            "pkg" => (None, &path[1..]),
            "core" => (ModulePath::from_str_segments(&["core"]), &path[1..]),
            "self" => (self.current.file_dir(self.current_is_dir), &path[1..]),
            "super" => {
                // `self` is the module's own directory; each `super` steps
                // one directory further up. Stepping above the package root
                // leaves the reference unresolved (the checker diagnoses).
                let supers = path.iter().take_while(|s| s.as_ref() == "super").count();
                let mut dir = self.current.file_dir(self.current_is_dir);
                for _ in 0..supers {
                    dir = dir?.parent();
                }
                (dir, &path[supers..])
            }
            // A module alias from a `use` item. Locals and module-level
            // declarations shadow aliases.
            _ => {
                if self.is_local(head) || self.module_types.contains(head) {
                    return None;
                }
                let alias = self.scope_module(head)?;
                (Some(alias.clone()), &path[1..])
            }
        };

        for segment in rest {
            cursor = Some(
                self.registry
                    .resolve_module_child(cursor.as_ref(), segment)?,
            );
        }
        cursor
    }
}

mod refs;
#[cfg(test)]
mod tests;
mod types;
mod walk;

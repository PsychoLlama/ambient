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

use deps::DepRecorder;

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
    // module to check. This routes through the same classified funnel as
    // every other edge, tagged [`RefPos::Import`] (check-only, `deps` only —
    // a `use` emits nothing at compile time; every *value* use of an imported
    // binding records its own link edge at the reference site). This loop
    // records the *module-level* imports; block-scoped `use` statements record
    // the same `RefPos::Import` edge as they bind their overlay (see
    // `bind_block_use` in `walk`), so any `use`, at any scope, is a dependency.
    for import in resolver.scope.items.values().flatten() {
        if &import.module != module_path {
            let id = registry.module_id(&import.module);
            resolver.deps.record(&id, RefPos::Import);
        }
    }
    // `link_deps ⊆ deps` holds by construction: [`DepRecorder::record`] is
    // the sole writer of both sets and never writes `link_deps` without also
    // writing `deps`, so there is nothing to assert at runtime.
    let (deps, link_deps) = resolver.deps.into_parts();
    ResolveOutcome {
        deps,
        link_deps,
        errors: resolver.import_errors,
    }
}

/// The syntactic position a resolved reference occupies. It is the sole
/// input to dep classification: [`DepRecorder::record`] writes `link_deps`
/// for [`Self::Value`] and `deps` alone for every other variant.
///
/// A value/symbol position links a compile-time artifact; a type or import
/// position emits nothing at link time, so it records the check-order `deps`
/// edge alone. Passing this explicitly — through the shared,
/// syntactically-ambiguous entry points (`resolve_path_ref` / `lookup_item`,
/// reached both by a qualified value call `pkg::m::foo` and a qualified
/// typed-record construction `pkg::m::Foo{..}`) *and* through the direct
/// funnel calls — makes the classification a required parameter rather than
/// a per-method-name convention.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RefPos {
    /// A value/symbol reference the compiler emits a link-time artifact for:
    /// a function/const call, an enum-variant or unit-struct construction, an
    /// ability perform/handler, or a module-alias method call. Writes both
    /// `deps` and `link_deps`.
    Value,
    /// A type-position reference — a typed-record construction (`Foo{..}`,
    /// whose lowering discards the type name) or a qualified dotted type path
    /// in an annotation/signature/impl target (`x: pkg::m::Foo`). The checker
    /// needs the defining module registered, but the compiler emits no link
    /// artifact, so it writes `deps` only.
    Type,
    /// A `use` import statement, at module scope or block scope alike. Its
    /// target must exist (and its enums/abilities register) for the module to
    /// check, but a `use` emits nothing at compile time, so it writes `deps`
    /// only — every *value* use of an imported binding records its own link
    /// edge at the reference site.
    Import,
}

/// What the resolve pass learned about a module.
pub struct ResolveOutcome {
    /// Foreign modules the module's references resolved into (as
    /// [`ModuleId`]s, ordered): its true dependency set, which the build
    /// uses for compilation ordering, cache keys, and `ModuleEnv` scoping.
    ///
    /// This is the full superset — every reference, value *and* type. It is
    /// what every current consumer reads.
    pub deps: BTreeSet<ModuleId>,
    /// The **link-order** subset of [`Self::deps`]: foreign modules reached
    /// through a value/symbol-position reference (function/const call,
    /// enum-variant or unit-struct construction, ability perform/handler,
    /// module-alias method call) — the edges whose compiled output the
    /// referencing module's bytecode must link against.
    ///
    /// Excludes the pure check-order edges: `use`-import statements,
    /// qualified type paths in annotations/signatures/impl targets, and
    /// typed-record *construction* (the compiler discards the type name), all
    /// of which the checker consumes but the compiler emits nothing against.
    /// `link_deps ⊆ deps` always holds.
    ///
    /// Its sole consumer is the compile-ordering graph
    /// ([`dispatch_ordering_graph`](crate::build::reachability::dispatch_ordering_graph)),
    /// which bases compile order on `link_deps ∪ structural dispatch edges`
    /// (not the full `deps`) so the self-orphan dispatch case links. Every
    /// other consumer — cache keys, `ModuleEnv` scoping, cycle detection,
    /// lazy reachability — reads the full [`Self::deps`].
    pub link_deps: BTreeSet<ModuleId>,
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
    /// The current module's package mount (`["foo"]` in a mounted build,
    /// `None` in the bare layout): where inline `pkg::` anchors and the
    /// floor `super::` may not step above.
    package_root: Option<ModulePath>,
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
    /// Module-level trait names: a bare trait reference (an impl header or a
    /// `T: Bound`) naming one canonicalizes to its own `Fqn(current,
    /// [name])`, the key the build-global trait table indexes it under.
    module_traits: HashSet<Arc<str>>,
    /// Lexical scope stack of local binding names (params, lets, pattern
    /// bindings, handler params). Locals shadow everything.
    locals: Vec<Vec<Arc<str>>>,
    /// Scope stack of in-scope type-parameter names (a declaration's
    /// `<T, U>` generics), innermost last. A bare type head that names one
    /// of these is a type parameter, not a nominal reference, and stays
    /// bare — mirroring the checker's `rigid_params`. Nesting composes: an
    /// impl block's params stay in scope for its methods' own params.
    type_params: Vec<Vec<Arc<str>>>,
    /// Import overlays from block-scoped `use` statements, innermost
    /// last. Consulted before the module scope; popped with their block.
    overlays: Vec<ModuleScope>,
    /// Failed block-scoped imports, surfaced through [`ResolveOutcome`].
    import_errors: Vec<ImportError>,
    /// The resolve pass's accumulated dependency sets — the full `deps`
    /// superset and its link-order `link_deps` subset — mutable only through
    /// the classified [`DepRecorder::record`] funnel. See
    /// [`ResolveOutcome::deps`] / [`ResolveOutcome::link_deps`].
    deps: DepRecorder,
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
        let mut module_traits = HashSet::new();
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
                ItemKind::Set(s) => {
                    // Sets live in the ability namespace: a bare `with MySet`
                    // resolves like a bare ability, then expands to members.
                    module_abilities.insert(Arc::clone(&s.name));
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
                ItemKind::Trait(t) => {
                    module_traits.insert(Arc::clone(&t.name));
                }
                ItemKind::Impl(_) | ItemKind::Use(_) => {}
            }
        }
        let current_is_dir = registry.get(current).is_some_and(|info| info.is_dir_module);
        let package_root = ModulePath::from_segments(registry.package_root_of(current));
        Self {
            registry,
            current,
            current_is_dir,
            package_root,
            scope,
            module_values,
            module_variants,
            module_types,
            module_abilities,
            module_traits,
            locals: Vec::new(),
            type_params: Vec::new(),
            overlays: Vec::new(),
            import_errors: Vec::new(),
            deps: DepRecorder::default(),
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

    /// Whether `name` is an in-scope type parameter of an enclosing
    /// declaration. A bare type head naming one is a `Type::Param`, not a
    /// nominal reference, so the resolve pass leaves it bare.
    pub(super) fn is_type_param(&self, name: &str) -> bool {
        self.type_params
            .iter()
            .any(|frame| frame.iter().any(|n| n.as_ref() == name))
    }

    // ─────────────────────────────────────────────────────────────────────
    // Reference resolution
    // ─────────────────────────────────────────────────────────────────────

    /// Build the [`Fqn`] identity for `ident` defined in `module`, recording
    /// the dependency edge classified by `pos` through the
    /// [`DepRecorder::record`] funnel (foreign modules only — same-module
    /// references record nothing). The single position-aware entry point:
    /// the shared, syntactically-ambiguous callers (`resolve_path_ref` /
    /// `lookup_item`, reached by both a qualified value call and a qualified
    /// typed-record construction) pass `pos` explicitly, and the
    /// position-specific wrappers [`Self::canonical_value`] /
    /// [`Self::canonical_type`] pin it, so the classification is a *required
    /// parameter* rather than a per-method-name convention.
    fn canonical(&mut self, module: &ModulePath, ident: Vec<Arc<str>>, pos: RefPos) -> Fqn {
        let module_id = self.registry.module_id(module);
        if module != self.current {
            self.deps.record(&module_id, pos);
        }
        Fqn::new(module_id, ident)
    }

    /// Build the [`Fqn`] identity for `ident` defined in `module`, recording
    /// a foreign module as a **link-order** dependency ([`RefPos::Value`],
    /// both `deps` and `link_deps`). Same-module references record nothing
    /// (use [`Self::same_module`] where the module is statically the current
    /// one).
    ///
    /// Every call site reaching it names a value/symbol reference the compiler
    /// emits a link-time artifact for — a function/const call, an enum-variant
    /// or unit-struct construction, an ability perform/handler, or a
    /// module-alias method call — so it pins [`RefPos::Value`] into the
    /// [`Self::canonical`] funnel. Pure type-position edges route through
    /// the check-only [`Self::canonical_type`].
    fn canonical_value(&mut self, module: &ModulePath, ident: Vec<Arc<str>>) -> Fqn {
        self.canonical(module, ident, RefPos::Value)
    }

    /// Build the [`Fqn`] identity for `ident` defined in `module`, recording
    /// a foreign module as a **check-order** dependency ([`RefPos::Type`],
    /// `deps` only, never `link_deps`). The type-position twin of
    /// [`Self::canonical_value`]: typed-record *construction* of a foreign
    /// type (`Foo{..}`) names it type-side, but the compiler lowers
    /// `TypedRecord` to a plain `MakeRecord` and discards the type name (the
    /// nominal identity is a compile-time concept), so it emits no link
    /// artifact against the type's module and must not manufacture a
    /// link-order edge. Same-module references record nothing.
    fn canonical_type(&mut self, module: &ModulePath, ident: Vec<Arc<str>>) -> Fqn {
        self.canonical(module, ident, RefPos::Type)
    }

    /// Build the [`Fqn`] for `ident` declared in the *current* module,
    /// recording nothing — a same-module reference is never a cross-module
    /// dependency. Call sites that always name the current module route here
    /// instead of `canonical_value(current, …)`, whose `module != current`
    /// guard makes the funnel call unreachable; the honest name documents
    /// "this records no dep" at the call site for future auditors.
    fn same_module(&self, ident: Vec<Arc<str>>) -> Fqn {
        Fqn::new(self.registry.module_id(self.current), ident)
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
            // A leading `::` (spelled as an empty head segment) roots at the
            // workspace: the following segment names a package mount,
            // exactly like `use ::pkg::…` resolution.
            "" => (None, &path[1..]),
            // `pkg` anchors at the module's own package root: the mount in
            // a mounted build, the registry root (`None`) in the bare
            // layout.
            "pkg" => (self.package_root.clone(), &path[1..]),
            "core" => (ModulePath::from_str_segments(&["core"]), &path[1..]),
            "self" => (self.current.file_dir(self.current_is_dir), &path[1..]),
            "super" => {
                // `self` is the module's own directory; each `super` steps
                // one directory further up. Stepping above the package root
                // leaves the reference unresolved (the checker diagnoses) —
                // in a mounted build the floor is the mount, not the
                // registry root, so `super` can never cross into a sibling
                // package.
                let supers = path.iter().take_while(|s| s.as_ref() == "super").count();
                let floor = self
                    .package_root
                    .as_ref()
                    .map_or(0, |root| root.segments().len());
                let mut dir = self.current.file_dir(self.current_is_dir);
                for _ in 0..supers {
                    let d = dir?;
                    if d.segments().len() <= floor {
                        return None;
                    }
                    dir = d.parent();
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

#[cfg(test)]
mod dep_classification_tests;
mod deps;
mod refs;
#[cfg(test)]
mod tests;
mod types;
mod walk;

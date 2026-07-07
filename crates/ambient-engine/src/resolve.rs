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
//! - an inline rooted path (`pkg::util::double`, `core::primitives::Number::sqrt`,
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

use crate::ast::{
    Expr, ExprKind, ItemKind, Module, Pattern, PatternKind, QualifiedName, Stmt, StmtKind,
};
use crate::fqn::{Fqn, ModuleId};
use crate::module_path::ModulePath;
use crate::module_registry::{
    ExportKind, ImportError, ItemImport, ModuleRegistry, ModuleScope, Namespace,
};
use crate::types::{NamedType, Type};

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

    fn resolve(&mut self, module: &mut Module) {
        // Split the borrow: the resolver holds no reference into `module`.
        for item in &mut module.items {
            match &mut item.kind {
                ItemKind::Function(f) => {
                    for ability in &mut f.abilities {
                        self.resolve_ability_ref(ability);
                    }
                    for param in &mut f.params {
                        if let Some(ty) = &mut param.ty {
                            self.resolve_type(ty);
                        }
                    }
                    if let Some(ty) = &mut f.ret_ty {
                        self.resolve_type(ty);
                    }
                    self.push_scope(f.params.iter().map(|p| Arc::clone(&p.name)).collect());
                    self.resolve_expr(&mut f.body);
                    self.pop_scope();
                }
                ItemKind::Const(c) => {
                    self.resolve_type(&mut c.ty);
                    self.resolve_expr(&mut c.value);
                }
                ItemKind::Ability(a) => {
                    for dep in &mut a.dependencies {
                        self.resolve_ability_ref(dep);
                    }
                    for method in &mut a.methods {
                        for (_, ty) in &mut method.params {
                            self.resolve_type(ty);
                        }
                        self.resolve_type(&mut method.ret_ty);
                    }
                }
                ItemKind::Impl(imp) => {
                    self.resolve_type(&mut imp.for_type);
                    for method in &mut imp.methods {
                        for ability in &mut method.abilities {
                            self.resolve_ability_ref(ability);
                        }
                        for param in &mut method.params {
                            if let Some(ty) = &mut param.ty {
                                self.resolve_type(ty);
                            }
                        }
                        if let Some(ty) = &mut method.ret_ty {
                            self.resolve_type(ty);
                        }
                        let mut names: Vec<Arc<str>> =
                            method.params.iter().map(|p| Arc::clone(&p.name)).collect();
                        if method.has_self {
                            names.push(Arc::from("self"));
                        }
                        self.push_scope(names);
                        self.resolve_expr(&mut method.body);
                        self.pop_scope();
                    }
                }
                ItemKind::Struct(s) => self.resolve_type(&mut s.ty),
                ItemKind::TypeAlias(t) => self.resolve_type(&mut t.ty),
                ItemKind::Enum(e) => {
                    for variant in &mut e.variants {
                        if let Some(payload) = &mut variant.payload {
                            self.resolve_type(payload);
                        }
                    }
                }
                ItemKind::Trait(t) => {
                    for method in &mut t.methods {
                        for (_, ty) in &mut method.params {
                            self.resolve_type(ty);
                        }
                        self.resolve_type(&mut method.ret_ty);
                    }
                }
                ItemKind::Use(_) => {}
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // Scopes
    // ─────────────────────────────────────────────────────────────────────

    fn push_scope(&mut self, names: Vec<Arc<str>>) {
        self.locals.push(names);
    }

    fn pop_scope(&mut self) {
        self.locals.pop();
    }

    fn declare_local(&mut self, name: Arc<str>) {
        if let Some(top) = self.locals.last_mut() {
            top.push(name);
        } else {
            self.locals.push(vec![name]);
        }
    }

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

    /// Resolve a value-namespace reference (function or constant).
    fn resolve_value_ref(&mut self, name: &mut QualifiedName) {
        if name.resolved.is_some() {
            return;
        }
        if name.path.is_empty() {
            // Locals stay bare — they are lexical bindings, not items.
            if self.is_local(&name.name) {
                return;
            }
            // A same-module enum variant resolves to its two-segment ident
            // `Fqn(current, [Enum, Variant])` — the key its constructor
            // scheme is bound under. (Runtime tags still live in the enum
            // registry, keyed by bare variant name.)
            if let Some(enum_name) = self.module_variants.get(&name.name) {
                let ident = vec![Arc::clone(enum_name), Arc::clone(&name.name)];
                let current = self.current;
                name.resolved = Some(self.canonical(current, ident));
                return;
            }
            // A same-module function, const, or unit-struct value resolves
            // to its own `Fqn(current, [name])`.
            if self.module_values.contains(&name.name) {
                let current = self.current;
                name.resolved = Some(self.canonical(current, vec![Arc::clone(&name.name)]));
                return;
            }
            if let Some(import) = self.scope_item(&name.name, Namespace::Value) {
                match import.kind {
                    ExportKind::Function | ExportKind::Const => {
                        let (module, item) = (import.module.clone(), import.name.clone());
                        name.resolved = Some(self.canonical(&module, vec![item]));
                        return;
                    }
                    // An imported enum variant resolves to its declaring
                    // enum's two-segment ident, mirroring the same-module
                    // case. The enum segment comes from the defining module.
                    ExportKind::EnumVariant => {
                        let (module, variant) = (import.module.clone(), import.name.clone());
                        if let Some(enum_name) = self.variant_enum(&module, &variant) {
                            name.resolved = Some(self.canonical(&module, vec![enum_name, variant]));
                        }
                        return;
                    }
                    _ => {}
                }
            }
            // A bare `Origin` imported via `use m::{Origin}` lives in the
            // type namespace (structs are types), but a unit struct is also a
            // value. Canonicalize it to `<module>::Origin` — the key the
            // checker bound its constructor scheme under — mirroring the
            // Function/Const canonicalization above.
            if let Some(import) = self.scope_item(&name.name, Namespace::Type)
                && import.kind == ExportKind::Struct
                && self.registry.is_unit_struct(&import.module, &import.name)
            {
                let (module, item) = (import.module.clone(), import.name.clone());
                name.resolved = Some(self.canonical(&module, vec![item]));
            }
            return;
        }

        // A path reference: resolve the head to a module cursor, walk the
        // middle segments as child modules, and confirm the final name is
        // an item the target module exports.
        self.resolve_path_ref(name);
    }

    /// Resolve a path-qualified reference against the module its path
    /// names, chasing `pub use` re-exports to the defining origin.
    fn resolve_path_ref(&mut self, name: &mut QualifiedName) {
        let Some(target) = self.resolve_module_prefix(&name.path) else {
            // The path's prefix didn't lead through modules. The one path
            // shape this rescues is the explicit-enum variant spelling
            // `m::Enum::Variant`, where the final *path* segment names an
            // enum rather than a module.
            self.resolve_explicit_enum_variant(name);
            return;
        };
        if target == *self.current {
            // A qualified self-reference (`pkg::this_module::foo`,
            // `self::foo`) canonicalizes to the current module's `Fqn` —
            // the same identity a bare same-module reference resolves to.
            // Whether `foo` is a declared item or an injected export
            // (an intrinsic like `core::collections::List::get`), the
            // env/intrinsic tables and linker key by this `Fqn`.
            name.resolved = Some(self.canonical(&target, vec![Arc::clone(&name.name)]));
            return;
        }
        if let Some(resolved) = self.lookup_item(&target, &name.name) {
            name.resolved = Some(resolved);
        }
    }

    /// Resolve an ability reference (effect rows, performs, handler arms,
    /// sandbox clauses, ability dependencies).
    fn resolve_ability_ref(&mut self, name: &mut QualifiedName) {
        if name.resolved.is_some() {
            return;
        }
        if name.path.is_empty() {
            // The builtin `Exception` stays bare (it has no declaring
            // module). A locally-declared ability resolves to its own
            // `Fqn(current, [name])`; imported abilities canonicalize to
            // their declaring module.
            if self.module_abilities.contains(&name.name) {
                let current = self.current;
                name.resolved = Some(self.canonical(current, vec![Arc::clone(&name.name)]));
                return;
            }
            if let Some(import) = self.scope_item(&name.name, Namespace::Ability) {
                let (module, item) = (import.module.clone(), import.name.clone());
                name.resolved = Some(self.canonical(&module, vec![item]));
            }
            return;
        }

        self.resolve_path_ref(name);
    }

    /// Resolve a type-namespace reference (typed record constructors).
    ///
    /// Same-module types resolve to their own `Fqn(current, [name])` — the
    /// key the checker binds the current module's type aliases under; only
    /// imported types canonicalize to their declaring module's `Fqn`. The
    /// `Type::Nominal` uuid remains the runtime/content identity; this
    /// `Fqn` is only the checker-side location key.
    fn resolve_type_ref(&mut self, name: &mut QualifiedName) {
        if name.resolved.is_some() {
            return;
        }
        if name.path.is_empty() {
            if self.module_types.contains(&name.name) {
                let current = self.current;
                name.resolved = Some(self.canonical(current, vec![Arc::clone(&name.name)]));
                return;
            }
            if let Some(import) = self.scope_item(&name.name, Namespace::Type) {
                let (module, item) = (import.module.clone(), import.name.clone());
                name.resolved = Some(self.canonical(&module, vec![item]));
            }
            return;
        }
        self.resolve_path_ref(name);
    }

    /// Resolve a dotted type reference inside a `types::Type` value.
    ///
    /// Type syntax lowers qualified names to dotted `Named` heads
    /// (`pkg.shapes.Money`, `types.Shape`); this rewrites each to the
    /// type it names:
    ///
    /// - an enum → `Named` with the enum's bare name and nominal uuid
    ///   (exactly what the enum's own constructors produce),
    /// - a non-generic type alias → the aliased type itself (`unique`
    ///   aliases are already wrapped in `Type::Nominal`, so identity
    ///   rides along),
    /// - a reference into the current module → the bare local spelling.
    ///
    /// Generic type aliases would need parameter substitution, which
    /// belongs to the checker; qualified references to them stay
    /// unresolved and surface as undefined-type errors for now.
    fn resolve_type(&mut self, ty: &mut Type) {
        match ty {
            Type::Named(n) => {
                for arg in &mut n.args {
                    self.resolve_type(arg);
                }
                if !n.name.contains("::") {
                    return;
                }
                let segments: Vec<Arc<str>> = n.name.split("::").map(Arc::from).collect();
                let Some((item, prefix)) = segments.split_last() else {
                    return;
                };
                let Some(target) = self.resolve_module_prefix(prefix) else {
                    return;
                };
                if target == *self.current {
                    // A self-reference by qualified path: the bare local
                    // name is the canonical spelling.
                    n.name = Arc::clone(item);
                    return;
                }
                // Visibility check (and re-export chasing) through the
                // ordinary symbol lookup.
                let Ok((_, origin)) = self.registry.lookup_symbol(&target, item) else {
                    return;
                };
                let Some(info) = self.registry.get(&origin) else {
                    return;
                };
                self.deps.insert(self.registry.module_id(&origin));
                for decl in &info.module.items {
                    match &decl.kind {
                        ItemKind::Enum(def) if def.name == *item => {
                            *ty = Type::Named(NamedType {
                                name: Arc::clone(&def.name),
                                args: std::mem::take(&mut n.args),
                                uuid: Some(def.uuid),
                            });
                            return;
                        }
                        ItemKind::Struct(def)
                            if def.name == *item && def.type_params.is_empty() =>
                        {
                            *ty = def.ty.clone();
                            return;
                        }
                        ItemKind::TypeAlias(def)
                            if def.name == *item && def.type_params.is_empty() =>
                        {
                            *ty = def.ty.clone();
                            return;
                        }
                        _ => {}
                    }
                }
            }
            Type::Tuple(elems) => {
                for elem in elems {
                    self.resolve_type(elem);
                }
            }
            Type::Record(rec) => {
                let fields = std::mem::take(&mut rec.fields);
                rec.fields = fields
                    .into_iter()
                    .map(|(name, mut field_ty)| {
                        self.resolve_type(&mut field_ty);
                        (name, field_ty)
                    })
                    .collect();
            }
            Type::Function(f) => {
                for param in &mut f.params {
                    self.resolve_type(param);
                }
                self.resolve_type(&mut f.ret);
            }
            Type::Nominal(n) => self.resolve_type(&mut n.inner),
            Type::Forall(forall) => self.resolve_type(&mut forall.body),
            _ => {}
        }
    }

    /// Look up `name` as an item of a *foreign* `module`, chasing `pub use`
    /// re-export chains to the defining origin, and land on its `Fqn`.
    /// Same-module references are normalized to bare by
    /// [`Self::resolve_path_ref`] before reaching here.
    ///
    /// A final segment that names an enum variant lands on the canonical
    /// two-segment ident `Fqn(enum_module, [Enum, Variant])` — the key its
    /// constructor scheme binds under — so `core::Option::Some` resolves
    /// exactly like the imported/bare spellings.
    fn lookup_item(&mut self, module: &ModulePath, name: &str) -> Option<Fqn> {
        let (export, origin) = self.registry.lookup_symbol(module, name).ok()?;
        let kind = export.kind;
        if kind == ExportKind::EnumVariant {
            let enum_name = self.variant_enum(&origin, name)?;
            return Some(self.canonical(&origin, vec![enum_name, Arc::from(name)]));
        }
        Some(self.canonical(&origin, vec![Arc::from(name)]))
    }

    /// Resolve the explicit-enum variant spelling `m::Enum::Variant`, where
    /// the last *path* segment (`Enum`) names an enum rather than a module,
    /// so [`Self::resolve_module_prefix`] over the full path fails.
    ///
    /// Tightly gated: the prefix minus the enum segment must name a module
    /// that publicly exports an enum of that name whose variants include
    /// the final ident. An empty prefix (`Enum::Variant`, `Money::default`)
    /// never qualifies — it is an associated path the checker owns.
    fn resolve_explicit_enum_variant(&mut self, name: &mut QualifiedName) {
        let Some((enum_seg, prefix)) = name.path.split_last() else {
            return;
        };
        if prefix.is_empty() {
            return;
        }
        let Some(target) = self.resolve_module_prefix(prefix) else {
            return;
        };
        let Ok((export, origin)) = self.registry.lookup_symbol(&target, enum_seg) else {
            return;
        };
        if export.kind != ExportKind::Enum {
            return;
        }
        let enum_name = Arc::clone(&export.name);
        if !self.enum_has_variant(&origin, &enum_name, &name.name) {
            return;
        }
        let variant = Arc::clone(&name.name);
        name.resolved = Some(self.canonical(&origin, vec![enum_name, variant]));
    }

    /// Whether `module` declares an enum named `enum_name` that has a
    /// variant `variant`.
    fn enum_has_variant(&self, module: &ModulePath, enum_name: &str, variant: &str) -> bool {
        self.registry.get(module).is_some_and(|info| {
            info.module.items.iter().any(|item| {
                matches!(&item.kind,
                    ItemKind::Enum(e) if e.name.as_ref() == enum_name
                        && e.variants.iter().any(|v| v.name.as_ref() == variant))
            })
        })
    }

    /// The enum that declares `variant` in foreign `module`, if any — the
    /// first ident segment of an imported variant's two-segment `Fqn`.
    fn variant_enum(&self, module: &ModulePath, variant: &str) -> Option<Arc<str>> {
        let info = self.registry.get(module)?;
        for item in &info.module.items {
            if let ItemKind::Enum(e) = &item.kind
                && e.variants.iter().any(|v| v.name.as_ref() == variant)
            {
                return Some(Arc::clone(&e.name));
            }
        }
        None
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

    // ─────────────────────────────────────────────────────────────────────
    // AST walking
    // ─────────────────────────────────────────────────────────────────────

    #[allow(clippy::too_many_lines)]
    fn resolve_expr(&mut self, expr: &mut Expr) {
        // Module-alias method-call disambiguation: `utils.helper(x)` parses
        // as a method call on the value `utils`, but when `utils` is a
        // module alias (and not a local), it is a qualified call — rewrite
        // the node so the checker and compiler see it that way.
        let rewrite = if let ExprKind::MethodCall {
            receiver,
            method,
            args,
            ..
        } = &mut expr.kind
        {
            if let ExprKind::Name(name) = &receiver.kind {
                let target = (name.path.is_empty()
                    && !self.is_local(&name.name)
                    && !self.module_values.contains(&name.name)
                    && !self.module_variants.contains_key(&name.name))
                .then(|| self.scope_module(&name.name).cloned())
                .flatten()
                .filter(|module| self.item_exists(module, method));
                target.map(|module| {
                    let mut callee_name =
                        QualifiedName::qualified(vec![Arc::clone(&name.name)], Arc::clone(method));
                    callee_name.resolved = Some(self.canonical(&module, vec![Arc::clone(method)]));
                    let callee = Expr {
                        kind: ExprKind::Name(callee_name),
                        span: receiver.span,
                        ty: None,
                    };
                    ExprKind::Call(Box::new(callee), std::mem::take(args))
                })
            } else {
                None
            }
        } else {
            None
        };
        if let Some(kind) = rewrite {
            expr.kind = kind;
        }

        match &mut expr.kind {
            ExprKind::Unit
            | ExprKind::Bool(_)
            | ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Local(_) => {}

            ExprKind::Name(name) => self.resolve_value_ref(name),

            ExprKind::Tuple(elems) | ExprKind::List(elems) => {
                for elem in elems {
                    self.resolve_expr(elem);
                }
            }
            ExprKind::TupleIndex(inner, _) | ExprKind::RecordField(inner, _) => {
                self.resolve_expr(inner);
            }
            ExprKind::Record(fields) => {
                for (_, value) in fields {
                    self.resolve_expr(value);
                }
            }
            ExprKind::TypedRecord { type_name, fields } => {
                self.resolve_type_ref(type_name);
                for (_, value) in fields {
                    self.resolve_expr(value);
                }
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                self.resolve_expr(receiver);
                for arg in args {
                    self.resolve_expr(arg);
                }
            }
            ExprKind::Binary { left, right, .. } => {
                self.resolve_expr(left);
                self.resolve_expr(right);
            }
            ExprKind::Unary(_, inner) | ExprKind::Resume(inner) => self.resolve_expr(inner),
            ExprKind::If(cond, then, els) => {
                self.resolve_expr(cond);
                self.resolve_expr(then);
                if let Some(els) = els {
                    self.resolve_expr(els);
                }
            }
            ExprKind::Match(scrutinee, arms) => {
                self.resolve_expr(scrutinee);
                for arm in arms {
                    let mut bindings = Vec::new();
                    collect_pattern_bindings(&arm.pattern, &mut bindings);
                    self.push_scope(bindings);
                    if let Some(guard) = &mut arm.guard {
                        self.resolve_expr(guard);
                    }
                    self.resolve_expr(&mut arm.body);
                    self.pop_scope();
                }
            }
            ExprKind::Block(stmts, result) => {
                self.push_scope(Vec::new());
                self.overlays.push(ModuleScope::default());
                for stmt in stmts {
                    self.resolve_stmt(stmt);
                }
                if let Some(result) = result {
                    self.resolve_expr(result);
                }
                self.overlays.pop();
                self.pop_scope();
            }
            ExprKind::Lambda(lambda) => {
                for param in &mut lambda.params {
                    if let Some(ty) = &mut param.ty {
                        self.resolve_type(ty);
                    }
                }
                if let Some(ty) = &mut lambda.ret_ty {
                    self.resolve_type(ty);
                }
                self.push_scope(lambda.params.iter().map(|p| Arc::clone(&p.name)).collect());
                self.resolve_expr(&mut lambda.body);
                self.pop_scope();
            }
            ExprKind::Call(callee, args) => {
                self.resolve_expr(callee);
                for arg in args {
                    self.resolve_expr(arg);
                }
            }
            ExprKind::Perform(call) => {
                self.resolve_ability_ref(&mut call.ability);
                for arg in &mut call.args {
                    self.resolve_expr(arg);
                }
            }
            ExprKind::Handle(handle) => {
                self.resolve_expr(&mut handle.body);
                for value in &mut handle.handler_values {
                    self.resolve_expr(value);
                }
                for handler in &mut handle.handlers {
                    self.resolve_ability_ref(&mut handler.ability);
                    self.push_scope(handler.params.iter().map(|p| Arc::clone(&p.name)).collect());
                    self.resolve_expr(&mut handler.body);
                    self.pop_scope();
                }
                if let Some(els) = &mut handle.else_clause {
                    self.resolve_expr(els);
                }
            }
            ExprKind::HandlerLiteral(literal) => {
                for method in &mut literal.methods {
                    self.push_scope(method.params.iter().map(|p| Arc::clone(&p.name)).collect());
                    self.resolve_expr(&mut method.body);
                    self.pop_scope();
                }
            }
            ExprKind::Sandbox(sandbox) => {
                for ability in &mut sandbox.allowed_abilities {
                    self.resolve_ability_ref(ability);
                }
                self.resolve_expr(&mut sandbox.body);
            }
        }
    }

    fn resolve_stmt(&mut self, stmt: &mut Stmt) {
        match &mut stmt.kind {
            StmtKind::Let(binding) => {
                if let Some(ty) = &mut binding.ty {
                    self.resolve_type(ty);
                }
                self.resolve_expr(&mut binding.init);
                self.declare_local(Arc::clone(&binding.name));
            }
            StmtKind::Expr(expr) => self.resolve_expr(expr),
            StmtKind::Use(use_def) => self.bind_block_use(use_def, stmt.span),
        }
    }

    /// Bind a block-scoped `use` into the innermost overlay. Semantics
    /// match a module-level `use`, except the binding ends with the
    /// enclosing block and `Local` heads may resolve through any visible
    /// scope (outer blocks, then the module scope).
    fn bind_block_use(&mut self, use_def: &crate::ast::UseDef, span: crate::ast::Span) {
        use crate::ast::UsePrefix;

        let path_names: Vec<Arc<str>> = use_def.path.iter().map(|(name, _)| name.clone()).collect();
        let target = if use_def.prefix == UsePrefix::Local {
            let Some(head) = path_names.first() else {
                return;
            };
            let Some(base) = self.scope_module(head).cloned() else {
                self.import_errors.push(ImportError {
                    error: crate::module_registry::RegistryError::UnresolvedHead {
                        head: head.to_string(),
                    },
                    span,
                });
                return;
            };
            let mut segments = base.segments().to_vec();
            segments.extend(path_names[1..].iter().cloned());
            match ModulePath::from_segments(segments) {
                Some(target) => target,
                None => return,
            }
        } else {
            match self
                .registry
                .resolve_use_path(self.current, &use_def.prefix, &path_names)
            {
                Ok(target) => target,
                Err(error) => {
                    self.import_errors.push(ImportError { error, span });
                    return;
                }
            }
        };

        let mut bound = ModuleScope::default();
        self.registry
            .bind_use_target(&mut bound, use_def, &target, span);
        self.import_errors.append(&mut bound.errors);
        if let Some(overlay) = self.overlays.last_mut() {
            for (name, module) in bound.modules {
                overlay.modules.insert(name, module);
            }
            for (name, imports) in bound.items {
                overlay.items.insert(name, imports);
            }
        }
    }
}

/// Collect the names a pattern binds.
fn collect_pattern_bindings(pattern: &Pattern, out: &mut Vec<Arc<str>>) {
    match &pattern.kind {
        PatternKind::Wildcard | PatternKind::Literal(_) => {}
        PatternKind::Binding(_, name) => out.push(Arc::clone(name)),
        PatternKind::Tuple(elems) => {
            for elem in elems {
                collect_pattern_bindings(elem, out);
            }
        }
        PatternKind::Record(fields) => {
            for (_, field) in fields {
                collect_pattern_bindings(field, out);
            }
        }
        PatternKind::Variant(_, payload) => {
            if let Some(payload) = payload {
                collect_pattern_bindings(payload, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{
        AbilityDef, ConstDef, EnumDef, EnumVariant, Expr, FunctionDef, Item, ItemKind, Module, Span,
    };
    use crate::types::Type;

    fn func(name: &str, body: Expr, abilities: Vec<QualifiedName>) -> Item {
        Item::new(
            ItemKind::Function(FunctionDef {
                name: Arc::from(name),
                name_span: Span::default(),
                is_public: true,
                type_params: vec![],
                params: vec![],
                ret_ty: None,
                abilities,
                body,
            }),
            Span::default(),
        )
    }

    /// Resolve module `m` (single-package registry) and return it.
    fn resolve_m(items: Vec<Item>) -> (Module, ModuleRegistry, ModulePath) {
        let mut module = Module {
            name: Arc::from("m"),
            doc: None,
            items,
        };
        let mut registry = ModuleRegistry::new();
        let path = ModulePath::from_str_segments(&["m"]).unwrap();
        registry.register(&path, Arc::new(module.clone()));
        resolve_module(&mut module, &path, &registry);
        (module, registry, path)
    }

    /// The `resolved` of the body `Name` of function `name`.
    fn body_resolved(module: &Module, name: &str) -> Option<Fqn> {
        module.items.iter().find_map(|item| match &item.kind {
            ItemKind::Function(f) if f.name.as_ref() == name => match &f.body.kind {
                crate::ast::ExprKind::Name(n) => n.resolved.clone(),
                _ => None,
            },
            _ => None,
        })
    }

    #[test]
    fn same_module_function_reference_resolves_to_its_fqn() {
        let items = vec![
            func("a", Expr::unit(), vec![]),
            func("ref_a", Expr::name("a"), vec![]),
        ];
        let (module, registry, path) = resolve_m(items);
        assert_eq!(
            body_resolved(&module, "ref_a"),
            Some(registry.fqn(&path, &[Arc::from("a")]))
        );
    }

    #[test]
    fn same_module_const_reference_resolves_to_its_fqn() {
        let konst = Item::new(
            ItemKind::Const(ConstDef {
                name: Arc::from("K"),
                name_span: Span::default(),
                is_public: true,
                ty: Type::number(),
                value: Expr::number(1.0),
            }),
            Span::default(),
        );
        let items = vec![konst, func("ref_k", Expr::name("K"), vec![])];
        let (module, registry, path) = resolve_m(items);
        assert_eq!(
            body_resolved(&module, "ref_k"),
            Some(registry.fqn(&path, &[Arc::from("K")]))
        );
    }

    #[test]
    fn same_module_enum_variant_resolves_to_two_segment_ident() {
        let enum_def = Item::new(
            ItemKind::Enum(EnumDef {
                name: Arc::from("E"),
                name_span: Span::default(),
                is_public: true,
                type_params: vec![],
                variants: vec![EnumVariant {
                    name: Arc::from("V"),
                    payload: None,
                    span: Span::default(),
                }],
                uuid: uuid::Uuid::from_u128(1),
            }),
            Span::default(),
        );
        let items = vec![enum_def, func("ref_v", Expr::name("V"), vec![])];
        let (module, registry, path) = resolve_m(items);
        // The variant's ident is `[Enum, Variant]`, not the bare `[V]`.
        assert_eq!(
            body_resolved(&module, "ref_v"),
            Some(registry.fqn(&path, &[Arc::from("E"), Arc::from("V")]))
        );
    }

    #[test]
    fn imported_enum_variant_resolves_to_two_segment_ident() {
        use crate::ast::{UseDef, UsePrefix};

        // Module `shapes` declares `enum Shape { Circle }`.
        let shapes = Module {
            name: Arc::from("shapes"),
            doc: None,
            items: vec![Item::new(
                ItemKind::Enum(EnumDef {
                    name: Arc::from("Shape"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    variants: vec![EnumVariant {
                        name: Arc::from("Circle"),
                        payload: None,
                        span: Span::default(),
                    }],
                    uuid: uuid::Uuid::from_u128(2),
                }),
                Span::default(),
            )],
        };
        // Module `main` imports the variant by name and references it.
        let use_item = Item::new(
            ItemKind::Use(UseDef {
                is_public: false,
                prefix: UsePrefix::Pkg,
                path: vec![
                    (Arc::from("shapes"), Span::default()),
                    (Arc::from("Circle"), Span::default()),
                ],
                alias: None,
            }),
            Span::default(),
        );
        let mut main = Module {
            name: Arc::from("main"),
            doc: None,
            items: vec![use_item, func("ref_circle", Expr::name("Circle"), vec![])],
        };

        let mut registry = ModuleRegistry::new();
        let shapes_path = ModulePath::from_str_segments(&["shapes"]).unwrap();
        let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
        registry.register(&shapes_path, Arc::new(shapes));
        registry.register(&main_path, Arc::new(main.clone()));
        resolve_module(&mut main, &main_path, &registry);

        // The imported variant lands on `shapes::Shape::Circle`, not a bare
        // `Circle` (corner #2: imported variants now resolve).
        assert_eq!(
            body_resolved(&main, "ref_circle"),
            Some(registry.fqn(&shapes_path, &[Arc::from("Shape"), Arc::from("Circle")]))
        );
    }

    /// Build a `shapes` module (`enum Shape { Circle, Dot }`) plus a `main`
    /// that references a variant through `callee`, resolve `main`, and return
    /// it with the registry and shapes path.
    fn shapes_and_ref(callee: QualifiedName) -> (Module, ModuleRegistry, ModulePath) {
        let shapes = Module {
            name: Arc::from("shapes"),
            doc: None,
            items: vec![Item::new(
                ItemKind::Enum(EnumDef {
                    name: Arc::from("Shape"),
                    name_span: Span::default(),
                    is_public: true,
                    type_params: vec![],
                    variants: vec![
                        EnumVariant {
                            name: Arc::from("Circle"),
                            payload: None,
                            span: Span::default(),
                        },
                        EnumVariant {
                            name: Arc::from("Dot"),
                            payload: None,
                            span: Span::default(),
                        },
                    ],
                    uuid: uuid::Uuid::from_u128(3),
                }),
                Span::default(),
            )],
        };
        let body = Expr {
            kind: crate::ast::ExprKind::Name(callee),
            span: Span::default(),
            ty: None,
        };
        let mut main = Module {
            name: Arc::from("main"),
            doc: None,
            items: vec![func("ref_circle", body, vec![])],
        };
        let mut registry = ModuleRegistry::new();
        let shapes_path = ModulePath::from_str_segments(&["shapes"]).unwrap();
        let main_path = ModulePath::from_str_segments(&["main"]).unwrap();
        registry.register(&shapes_path, Arc::new(shapes));
        registry.register(&main_path, Arc::new(main.clone()));
        resolve_module(&mut main, &main_path, &registry);
        (main, registry, shapes_path)
    }

    #[test]
    fn foreign_variant_qualified_by_module_resolves_to_two_segment_ident() {
        // `pkg::shapes::Circle` — the variant is a direct export of module
        // `shapes` — lands on the two-segment `shapes::Shape::Circle`.
        let (main, registry, shapes_path) =
            shapes_and_ref(QualifiedName::qualified(vec!["pkg", "shapes"], "Circle"));
        assert_eq!(
            body_resolved(&main, "ref_circle"),
            Some(registry.fqn(&shapes_path, &[Arc::from("Shape"), Arc::from("Circle")]))
        );
    }

    #[test]
    fn foreign_variant_qualified_by_enum_resolves_to_two_segment_ident() {
        // `pkg::shapes::Shape::Circle` — the explicit-enum spelling, where the
        // last path segment names the enum, not a module — lands on the same
        // two-segment ident (via the `resolve_explicit_enum_variant` fallback).
        let (main, registry, shapes_path) = shapes_and_ref(QualifiedName::qualified(
            vec!["pkg", "shapes", "Shape"],
            "Circle",
        ));
        assert_eq!(
            body_resolved(&main, "ref_circle"),
            Some(registry.fqn(&shapes_path, &[Arc::from("Shape"), Arc::from("Circle")]))
        );
    }

    #[test]
    fn same_module_ability_reference_resolves_to_its_fqn() {
        let ability = Item::new(
            ItemKind::Ability(AbilityDef {
                name: Arc::from("A"),
                name_span: Span::default(),
                is_public: true,
                dependencies: vec![],
                methods: vec![],
                resolved_id: None,
            }),
            Span::default(),
        );
        let items = vec![
            ability,
            func("with_a", Expr::unit(), vec![QualifiedName::simple("A")]),
        ];
        let (module, registry, path) = resolve_m(items);
        let resolved = module.items.iter().find_map(|item| match &item.kind {
            ItemKind::Function(f) if f.name.as_ref() == "with_a" => {
                f.abilities.first().and_then(|q| q.resolved.clone())
            }
            _ => None,
        });
        assert_eq!(resolved, Some(registry.fqn(&path, &[Arc::from("A")])));
    }
}

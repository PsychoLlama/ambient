//! Reference-resolution rules for the resolve pass: mapping each value,
//! type, and ability *spelling* to the `Fqn` it names. This is the file
//! to open to learn the language's name-resolution policy.

use std::sync::Arc;

use crate::ast::{ItemKind, QualifiedName};
use crate::fqn::Fqn;
use crate::module_path::ModulePath;
use crate::module_registry::{ExportKind, Namespace};

use super::{RefPos, Resolver};

impl Resolver<'_> {
    /// Resolve a value-namespace reference (function or constant).
    pub(super) fn resolve_value_ref(&mut self, name: &mut QualifiedName) {
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
                name.resolved = Some(self.same_module(ident));
                return;
            }
            // A same-module function, const, or unit-struct value resolves
            // to its own `Fqn(current, [name])`.
            if self.module_values.contains(&name.name) {
                name.resolved = Some(self.same_module(vec![Arc::clone(&name.name)]));
                return;
            }
            if let Some(import) = self.scope_item(&name.name, Namespace::Value) {
                match import.kind {
                    ExportKind::Function | ExportKind::Const => {
                        let (module, item) = (import.module.clone(), import.name.clone());
                        name.resolved = Some(self.canonical_value(&module, vec![item]));
                        return;
                    }
                    // An imported enum variant resolves to its declaring
                    // enum's two-segment ident, mirroring the same-module
                    // case. The enum segment comes from the defining module.
                    ExportKind::EnumVariant => {
                        let (module, variant) = (import.module.clone(), import.name.clone());
                        if let Some(enum_name) = self.variant_enum(&module, &variant) {
                            name.resolved =
                                Some(self.canonical_value(&module, vec![enum_name, variant]));
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
                name.resolved = Some(self.canonical_value(&module, vec![item]));
            }
            return;
        }

        // A path reference: resolve the head to a module cursor, walk the
        // middle segments as child modules, and confirm the final name is
        // an item the target module exports. A qualified call/const is a
        // value position — a link-order edge.
        self.resolve_path_ref(name, RefPos::Value);
    }

    /// Resolve a path-qualified reference against the module its path
    /// names, chasing `pub use` re-exports to the defining origin.
    ///
    /// Shared between value and type positions: a qualified call/const
    /// (`pkg::m::foo`, value) and a qualified typed-record construction
    /// (`pkg::m::Foo{..}`, type — check-only) both land here. `pos` carries
    /// that classification through to [`Self::lookup_item`], the single
    /// ambiguous dep-recording site, so neither spelling gets a
    /// spelling-specific code path.
    fn resolve_path_ref(&mut self, name: &mut QualifiedName, pos: RefPos) {
        let Some(target) = self.resolve_module_prefix(&name.path) else {
            // The path's prefix didn't lead through modules. The one path
            // shape this rescues is the explicit-enum variant spelling
            // `m::Enum::Variant`, where the final *path* segment names an
            // enum rather than a module — always a value/construction site.
            self.resolve_explicit_enum_variant(name);
            return;
        };
        if target == *self.current {
            // A qualified self-reference (`pkg::this_module::foo`,
            // `self::foo`) canonicalizes to the current module's `Fqn` —
            // the same identity a bare same-module reference resolves to,
            // recording nothing. Whether `foo` is a declared item or an
            // injected export (an intrinsic like
            // `core::collections::list::get`), the env/intrinsic tables and
            // linker key by this `Fqn`.
            name.resolved = Some(self.same_module(vec![Arc::clone(&name.name)]));
            return;
        }
        if let Some(resolved) = self.lookup_item(&target, &name.name, pos) {
            name.resolved = Some(resolved);
        }
    }

    /// Resolve an ability reference (effect rows, performs, handler arms,
    /// sandbox clauses, ability dependencies).
    ///
    /// VALUE (records a link-order dep via `canonical_value`): a perform or
    /// handler links the ability's method-identity / default-implementation
    /// dispatch channel, so an imported ability reached here is a genuine
    /// link edge. An ability that is *only* imported and never referenced
    /// records its edge through the check-only `use`-loop instead — correct,
    /// since it emits no dispatch artifact.
    pub(super) fn resolve_ability_ref(&mut self, name: &mut QualifiedName) {
        if name.resolved.is_some() {
            return;
        }
        if name.path.is_empty() {
            // A locally-declared ability resolves to its own
            // `Fqn(current, [name])`; imported and prelude-injected
            // abilities (including `Exception`, re-exported from
            // `core::exception`) canonicalize to their declaring module.
            if self.module_abilities.contains(&name.name) {
                name.resolved = Some(self.same_module(vec![Arc::clone(&name.name)]));
                return;
            }
            if let Some(import) = self.scope_item(&name.name, Namespace::Ability) {
                let (module, item) = (import.module.clone(), import.name.clone());
                name.resolved = Some(self.canonical_value(&module, vec![item]));
            }
            return;
        }

        // A qualified perform links the ability's dispatch channel — value.
        self.resolve_path_ref(name, RefPos::Value);
    }

    /// Resolve a bare-method perform (`seed!(…)`, no spelled ability)
    /// through the imported-ability-method scope channel
    /// (`use m::Random::seed;`, block-scoped included). On a hit the call
    /// is canonicalized in place: the ability reference is synthesized
    /// with the declaring module's `Fqn` (no spelled spans) and the method
    /// rewritten to its declared name (an `as` alias vanishes here), so
    /// everything downstream — checker, compiler, interface hashing — sees
    /// exactly what the qualified spelling resolves to. A miss leaves
    /// `ability` as `None` and the checker diagnoses it.
    ///
    /// VALUE edge, like every perform: the site links the ability's
    /// method-identity / default-implementation dispatch channel.
    pub(super) fn resolve_bare_method_perform(&mut self, call: &mut crate::ast::AbilityCall) {
        let Some(import) = self.scope_item(&call.method, Namespace::AbilityMethod) else {
            return;
        };
        let Some(owner) = import.owner.clone() else {
            return;
        };
        let (module, method) = (import.module.clone(), import.name.clone());
        let fqn = self.canonical_value(&module, vec![Arc::clone(&owner)]);
        call.method = method;
        call.ability = Some(QualifiedName {
            path: vec![],
            path_spans: vec![],
            name: owner,
            name_span: None,
            resolved: Some(fqn),
        });
    }

    /// Resolve a trait reference (an impl header `impl Show for X`, or a
    /// bound `T: Show` / `where T: Show`) to the trait's `Fqn`.
    ///
    /// Traits are nominal — their `unique(<uuid>)` prefix is the identity —
    /// so this `Fqn` is only a build-global *lookup key* (the checker maps it
    /// to the uuid through `TraitRegistry`), never a content input. Bare
    /// names follow the same local→import→prelude precedence every other
    /// reference obeys; a qualified spelling (`some::module::Show`) resolves
    /// through the named module. Unlike a value reference this adds **no
    /// compile-ordering dependency**: a bound needs only the trait's
    /// *definition* registered (which happens upfront for every module),
    /// never its compiled body, so a trait edge here would manufacture
    /// spurious cycles — exactly the reasoning [`Resolver::resolve_type`]
    /// applies to bare type references.
    pub(super) fn resolve_trait_ref(&mut self, name: &mut QualifiedName) {
        if name.resolved.is_some() {
            return;
        }
        if name.path.is_empty() {
            // A locally-declared trait resolves to its own `Fqn(current,
            // [name])`; an imported or prelude trait to its declaring module.
            if self.module_traits.contains(&name.name) {
                let module_id = self.registry.module_id(self.current);
                name.resolved = Some(Fqn::new(module_id, vec![Arc::clone(&name.name)]));
                return;
            }
            if let Some(import) = self.scope_item(&name.name, Namespace::Trait) {
                let module_id = self.registry.module_id(&import.module);
                name.resolved = Some(Fqn::new(module_id, vec![Arc::clone(&import.name)]));
            }
            return;
        }
        // A qualified spelling: resolve the module prefix, then confirm the
        // final segment is a trait that module exports (chasing `pub use`
        // re-exports to the defining origin).
        let Some(target) = self.resolve_module_prefix(&name.path) else {
            return;
        };
        if let Ok((export, origin)) = self.registry.lookup_symbol(&target, &name.name)
            && export.kind == ExportKind::Trait
        {
            let module_id = self.registry.module_id(&origin);
            name.resolved = Some(Fqn::new(module_id, vec![Arc::clone(&name.name)]));
        }
    }

    /// Resolve a type-namespace reference (typed record constructors,
    /// `Foo{..}`).
    ///
    /// Same-module types resolve to their own `Fqn(current, [name])` — the
    /// key the checker binds the current module's type aliases under; only
    /// imported types canonicalize to their declaring module's `Fqn`. The
    /// `Type::Nominal` uuid remains the runtime/content identity; this
    /// `Fqn` is only the checker-side location key.
    ///
    /// CHECK-ONLY (records a `deps`-only edge via `canonical_type`, never
    /// `link_deps`): this is the only caller (from `walk`'s
    /// `ExprKind::TypedRecord`), and a typed record is *construction* of a
    /// nominal type spelled type-side. Although it looks like a value
    /// position, the compiler emits **no** link artifact for it:
    /// `compiler::expr` lowers `TypedRecord` to a plain `MakeRecord` over
    /// structural field pairs and discards `type_name` entirely (the nominal
    /// identity is a compile-time concept), so a foreign struct construction
    /// references nothing in the type's module at link time. Recording a link
    /// edge here would let the candidate-skip in
    /// [`dispatch_ordering_graph`](crate::build::reachability::dispatch_ordering_graph)
    /// drop a genuinely-needed self-orphan dispatch edge. Both spellings
    /// agree by construction: the bare/imported spelling routes through
    /// `canonical_type` below, and the qualified `pkg::m::Foo{..}` spelling
    /// threads [`RefPos::Type`] through the shared `resolve_path_ref` /
    /// `lookup_item`.
    pub(super) fn resolve_type_ref(&mut self, name: &mut QualifiedName) {
        if name.resolved.is_some() {
            return;
        }
        if name.path.is_empty() {
            if self.module_types.contains(&name.name) {
                name.resolved = Some(self.same_module(vec![Arc::clone(&name.name)]));
                return;
            }
            if let Some(import) = self.scope_item(&name.name, Namespace::Type) {
                let (module, item) = (import.module.clone(), import.name.clone());
                name.resolved = Some(self.canonical_type(&module, vec![item]));
            }
            return;
        }
        self.resolve_path_ref(name, RefPos::Type);
    }

    /// Look up `name` as an item of a *foreign* `module`, chasing `pub use`
    /// re-export chains to the defining origin, and land on its `Fqn`.
    /// Same-module references are normalized to bare by
    /// [`Self::resolve_path_ref`] before reaching here.
    ///
    /// A final segment that names an enum variant lands on the canonical
    /// two-segment ident `Fqn(enum_module, [Enum, Variant])` — the key its
    /// constructor scheme binds under — so `core::option::Some` resolves
    /// exactly like the imported/bare spellings.
    ///
    /// Records the dep edge classified by `pos` (via [`Self::canonical`]),
    /// the single ambiguous site `resolve_path_ref` funnels both spellings
    /// through: `resolve_value_ref`/`resolve_ability_ref` pass
    /// [`RefPos::Value`] (a qualified call/const `pkg::m::foo`, a qualified
    /// perform), while `resolve_type_ref` passes [`RefPos::Type`] (a
    /// qualified typed-record construction `pkg::m::Foo{..}`, whose compiler
    /// lowering emits no link artifact — see `resolve_type_ref`). A qualified
    /// type *annotation* (`x: pkg::m::Foo`) never reaches this path — it is
    /// resolved in `types` (check-only). An enum-variant final segment is
    /// always a value construction, so it stays link-order regardless of
    /// `pos` (a typed record can never name a variant).
    fn lookup_item(&mut self, module: &ModulePath, name: &str, pos: RefPos) -> Option<Fqn> {
        let (export, origin) = self.registry.lookup_symbol(module, name).ok()?;
        let kind = export.kind;
        if kind == ExportKind::EnumVariant {
            let enum_name = self.variant_enum(&origin, name)?;
            return Some(self.canonical_value(&origin, vec![enum_name, Arc::from(name)]));
        }
        Some(self.canonical(&origin, vec![Arc::from(name)], pos))
    }

    /// Resolve the explicit-enum variant spelling `m::Enum::Variant`, where
    /// the last *path* segment (`Enum`) names an enum rather than a module,
    /// so [`Self::resolve_module_prefix`] over the full path fails.
    ///
    /// Tightly gated: the prefix minus the enum segment must name a module
    /// that publicly exports an enum of that name whose variants include
    /// the final ident. An empty prefix (`Enum::Variant`) names an enum in
    /// the current scope — a local declaration or an imported enum — and is
    /// handled by [`Self::resolve_scoped_enum_variant`]; a `Type::method`
    /// path (whose head is a struct/trait, not an enum) is left `resolved`
    /// `None` for the checker's associated-path handling.
    fn resolve_explicit_enum_variant(&mut self, name: &mut QualifiedName) {
        let Some((enum_seg, prefix)) = name.path.split_last() else {
            return;
        };
        if prefix.is_empty() {
            let enum_seg = Arc::clone(enum_seg);
            self.resolve_scoped_enum_variant(&enum_seg, name);
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
        name.resolved = Some(self.canonical_value(&origin, vec![enum_name, variant]));
    }

    /// Resolve the `Enum::Variant` spelling where `Enum` names an enum in
    /// the current scope — a local declaration or one brought in by `use`.
    ///
    /// Lands on the same canonical two-segment ident `Fqn(enum_module,
    /// [Enum, Variant])` as the module-qualified (`m::Enum::Variant`) and
    /// bare spellings, so the checker and compiler resolve it by identity,
    /// never by a bare-name reverse lookup that a same-named local variant
    /// could hijack. Leaves `resolved` `None` when `enum_seg` is not an enum
    /// carrying variant `name` (e.g. the associated path `Money::default`),
    /// which the checker owns.
    fn resolve_scoped_enum_variant(&mut self, enum_seg: &Arc<str>, name: &mut QualifiedName) {
        // A local enum resolves into the current module.
        if self.enum_has_variant(self.current, enum_seg, &name.name) {
            let ident = vec![Arc::clone(enum_seg), Arc::clone(&name.name)];
            name.resolved = Some(self.same_module(ident));
            return;
        }
        // An imported enum resolves into its defining module.
        if let Some(import) = self.scope_item(enum_seg, Namespace::Type)
            && import.kind == ExportKind::Enum
        {
            let (module, enum_name) = (import.module.clone(), import.name.clone());
            if self.enum_has_variant(&module, &enum_name, &name.name) {
                let ident = vec![enum_name, Arc::clone(&name.name)];
                name.resolved = Some(self.canonical_value(&module, ident));
            }
        }
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
}

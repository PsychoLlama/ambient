//! Reference-resolution rules for the resolve pass: mapping each value,
//! type, and ability *spelling* to the `Fqn` it names. This is the file
//! to open to learn the language's name-resolution policy.

use std::sync::Arc;

use crate::ast::{ItemKind, QualifiedName};
use crate::fqn::Fqn;
use crate::module_path::ModulePath;
use crate::module_registry::{ExportKind, Namespace};

use super::Resolver;

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
                let current = self.current;
                name.resolved = Some(self.canonical_value(current, ident));
                return;
            }
            // A same-module function, const, or unit-struct value resolves
            // to its own `Fqn(current, [name])`.
            if self.module_values.contains(&name.name) {
                let current = self.current;
                name.resolved = Some(self.canonical_value(current, vec![Arc::clone(&name.name)]));
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
            // (an intrinsic like `core::collections::list::get`), the
            // env/intrinsic tables and linker key by this `Fqn`.
            name.resolved = Some(self.canonical_value(&target, vec![Arc::clone(&name.name)]));
            return;
        }
        if let Some(resolved) = self.lookup_item(&target, &name.name) {
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
                let current = self.current;
                name.resolved = Some(self.canonical_value(current, vec![Arc::clone(&name.name)]));
                return;
            }
            if let Some(import) = self.scope_item(&name.name, Namespace::Ability) {
                let (module, item) = (import.module.clone(), import.name.clone());
                name.resolved = Some(self.canonical_value(&module, vec![item]));
            }
            return;
        }

        self.resolve_path_ref(name);
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
    /// VALUE (records a link-order dep via `canonical_value`): this is the
    /// only caller (from `walk`'s `ExprKind::TypedRecord`), and a typed
    /// record is *construction* of a nominal type, spelled type-side. This
    /// is classified value **conservatively**, not because the compiler
    /// emits a link artifact for it — the phase-2 investigation found it does
    /// not: `compiler::expr` lowers `TypedRecord` to a plain `MakeRecord`
    /// over structural field pairs and discards `type_name` entirely (the
    /// nominal identity is a compile-time concept), so a foreign struct
    /// construction references nothing in the type's module at link time.
    /// It could therefore be reclassified CHECK-ONLY in phase 3; doing so
    /// would require threading position through the shared `resolve_path_ref`
    /// (used here for the qualified `pkg::m::Foo{..}` spelling) so the
    /// qualified and bare/imported spellings agree. Kept VALUE for now per
    /// the conservative-toward-value rule — over-including an edge is safe;
    /// dropping a needed one silently breaks linking.
    pub(super) fn resolve_type_ref(&mut self, name: &mut QualifiedName) {
        if name.resolved.is_some() {
            return;
        }
        if name.path.is_empty() {
            if self.module_types.contains(&name.name) {
                let current = self.current;
                name.resolved = Some(self.canonical_value(current, vec![Arc::clone(&name.name)]));
                return;
            }
            if let Some(import) = self.scope_item(&name.name, Namespace::Type) {
                let (module, item) = (import.module.clone(), import.name.clone());
                name.resolved = Some(self.canonical_value(&module, vec![item]));
            }
            return;
        }
        self.resolve_path_ref(name);
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
    /// VALUE (records a link-order dep via `canonical_value`): reached only
    /// through `resolve_path_ref`, whose every caller is a value position —
    /// `resolve_value_ref` (a qualified call/const `pkg::m::foo`),
    /// `resolve_ability_ref` (a qualified perform), and `resolve_type_ref`
    /// (a qualified typed-record construction `pkg::m::Foo{..}`). The
    /// earlier map flagged the "value or type reached by qualified path"
    /// ambiguity here; it does not arise. A qualified type *annotation*
    /// (`x: pkg::m::Foo`) never reaches this path — it is resolved in
    /// `types` (check-only), never `resolve_type_ref`. So every hit here is
    /// a genuine value/symbol position, not merely a conservative default.
    fn lookup_item(&mut self, module: &ModulePath, name: &str) -> Option<Fqn> {
        let (export, origin) = self.registry.lookup_symbol(module, name).ok()?;
        let kind = export.kind;
        if kind == ExportKind::EnumVariant {
            let enum_name = self.variant_enum(&origin, name)?;
            return Some(self.canonical_value(&origin, vec![enum_name, Arc::from(name)]));
        }
        Some(self.canonical_value(&origin, vec![Arc::from(name)]))
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
            let current = self.current;
            let ident = vec![Arc::clone(enum_seg), Arc::clone(&name.name)];
            name.resolved = Some(self.canonical_value(current, ident));
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

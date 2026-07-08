//! The per-module view of the rest of the build.
//!
//! [`ModuleEnv`] is the single derivation of "what does this module see?":
//! every cross-module fact a compile needs — imported enums, foreign enum
//! variants, unit structs, constant hashes, ability identities — is
//! extracted from the [`ModuleRegistry`] here and nowhere else. The
//! checker's import registration shares the same walks
//! ([`imported_enum_defs`], [`imported_trait_defs`]), so the checker and
//! the compiler agree on what a module imports *by construction*.
//!
//! History note: before this module, six call sites (the package build ×3,
//! the CLI single-file path, and two test harnesses) each hand-assembled
//! these channels, and they drifted — core compiles silently received a
//! subset. If you need a new cross-module channel, add it to [`ModuleEnv`];
//! never wire it at a call site.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{EnumDef, ItemKind, TraitDef};
use crate::fqn::{Fqn, ModuleId, NameKey};
use crate::module_path::ModulePath;
use crate::module_registry::{ExportKind, ModuleRegistry, ResolvedImport};

/// An enum variant's construction layout: what the compiler needs to inline
/// a constructor (`MakeEnum` tag + payload flag) rather than link by hash.
#[derive(Debug, Clone)]
pub struct VariantInfo {
    pub enum_name: Arc<str>,
    pub tag: u16,
    pub has_payload: bool,
}

/// A module's resolved view of the rest of the build, in compiler shape.
///
/// The zero-value [`Default`] is the registry-less convention
/// (single-file and unit-test compiles): no module identity, no imports —
/// matching a world where the resolve pass never ran.
#[derive(Default)]
pub struct ModuleEnv {
    /// The module's own identity. `Some` when compiling inside a registry
    /// (own items key on their [`Fqn`], matching the resolve pass); `None`
    /// in the registry-less convention, where same-module references stay
    /// bare.
    pub module_id: Option<ModuleId>,
    /// Enum definitions this module imports — explicit `use` or the prelude
    /// (`Option`/`Result`). Constructors inline by tag, so the compiler
    /// needs the definitions themselves, not name→hash entries.
    pub imported_enums: Vec<EnumDef>,
    /// Every public foreign enum's variants, keyed by their canonical
    /// two-segment [`Fqn`] (`core::Option::Some`). The *qualified* channel:
    /// [`Self::imported_enums`] covers bare references; fully-qualified
    /// references — which need no import — are looked up here.
    pub foreign_enum_variants: Vec<(Fqn, VariantInfo)>,
    /// Every public foreign unit struct, as its canonical key. A unit
    /// struct compiles to an empty record value, so only the key is needed.
    pub foreign_unit_structs: Vec<NameKey>,
    /// Every public foreign constant's value-object hash, keyed by
    /// canonical [`Fqn`]. A `const` links by hash like a function: a
    /// reference compiles to a `LoadObject` of the hash, and the defining
    /// module ships the (content-addressed, deduplicated) value object.
    pub foreign_const_hashes: HashMap<NameKey, blake3::Hash>,
    /// Every registered module's `ability` declarations resolved to their
    /// content-addressed identities, keyed by [`Fqn`]. Identity is the
    /// interface hash, so recomputing here matches the declaring module's
    /// own registration exactly.
    pub foreign_abilities: Vec<(Fqn, Arc<crate::ability_resolver::DynAbility>)>,
    /// The host's native bindings for *this* module's `extern fn`
    /// declarations, keyed by item name. The compiler builds each
    /// declaration's [`Native` object](crate::object::StoredObject::Native)
    /// from its binding; a declaration with no entry here is an
    /// unbound-extern compile error. Foreign extern fns need no channel of
    /// their own: their hashes travel through the ordinary linking table
    /// like any compiled function's.
    pub extern_natives: HashMap<Arc<str>, crate::natives::NativeKey>,
}

impl ModuleEnv {
    /// Derive `module_path`'s view of the build from the registry.
    ///
    /// This is the only sanctioned way to wire a registry-backed compile:
    /// [`crate::compiler::CompileOptions`] takes the whole env, so a module
    /// can never be compiled with a partial view of the build.
    #[must_use]
    pub fn new(registry: &ModuleRegistry, module_path: &ModulePath) -> Self {
        let mut foreign_enum_variants = Vec::new();
        let mut foreign_unit_structs = Vec::new();
        let mut foreign_const_hashes = HashMap::new();

        // One walk over the foreign modules collects every "all public
        // items" channel. Foreign items are provided whether or not they
        // are imported: inline qualified references (`pkg::shapes::Circle`)
        // need no `use`.
        for info in registry.all_modules() {
            if &info.path == module_path {
                continue;
            }
            for item in &info.module.items {
                match &item.kind {
                    ItemKind::Enum(def) if def.is_public => {
                        for (idx, variant) in def.variants.iter().enumerate() {
                            let fqn = registry.fqn(
                                &info.path,
                                &[Arc::clone(&def.name), Arc::clone(&variant.name)],
                            );
                            foreign_enum_variants.push((
                                fqn,
                                VariantInfo {
                                    enum_name: Arc::clone(&def.name),
                                    #[allow(clippy::cast_possible_truncation)]
                                    tag: idx as u16,
                                    has_payload: variant.payload.is_some(),
                                },
                            ));
                        }
                    }
                    ItemKind::Struct(def) if def.is_public && def.is_unit() => {
                        foreign_unit_structs.push(NameKey::Item(
                            registry.fqn(&info.path, &[Arc::clone(&def.name)]),
                        ));
                    }
                    ItemKind::Const(def) if def.is_public => {
                        // A value object's hash derives only from the value,
                        // never its name, so this recomputes exactly the hash
                        // the defining module's own compile produced.
                        if let Some(value) = crate::const_eval::literal_value(&def.value)
                            && let Ok(object) = crate::object::value_object(&value)
                        {
                            let key =
                                NameKey::Item(registry.fqn(&info.path, &[Arc::clone(&def.name)]));
                            foreign_const_hashes.insert(key, object.hash());
                        }
                    }
                    _ => {}
                }
            }
        }

        Self {
            module_id: Some(registry.module_id(module_path)),
            imported_enums: imported_enum_defs(registry, module_path)
                .into_iter()
                .map(|(_, def)| def)
                .collect(),
            foreign_enum_variants,
            foreign_unit_structs,
            foreign_const_hashes,
            foreign_abilities: crate::infer::resolve_registry_abilities(registry),
            extern_natives: registry.natives().keys_for_module(module_path),
        }
    }
}

/// The enum definitions a module imports — by explicit `use` or through the
/// prelude — paired with the defining module's identity.
///
/// This walk is shared between the checker (`register_imported_enums`) and
/// [`ModuleEnv::new`]: both read `resolve_imports`, which folds in the
/// prelude re-exports, so the compiler receives exactly the enums the
/// checker registered — `Option`/`Result` included. That shared walk is
/// what lets prelude enum construction compile without a hardcoded seed.
#[must_use]
pub fn imported_enum_defs(
    registry: &ModuleRegistry,
    module_path: &ModulePath,
) -> Vec<(ModuleId, EnumDef)> {
    let mut enums = Vec::new();
    let Ok(resolved) = registry.resolve_imports(module_path) else {
        return enums;
    };
    for (name, bindings) in resolved.imports {
        for import in bindings {
            let ResolvedImport::Symbol {
                from_module,
                export_kind: ExportKind::Enum,
                ..
            } = import
            else {
                continue;
            };
            if let Some(info) = registry.get(&from_module) {
                let enum_module = registry.module_id(&from_module);
                for item in &info.module.items {
                    if let ItemKind::Enum(def) = &item.kind
                        && def.name == name
                    {
                        enums.push((enum_module.clone(), def.clone()));
                    }
                }
            }
        }
    }
    enums
}

/// The trait definitions a module imports — by explicit `use` or through
/// the prelude (the operator traits). The checker registers exactly these,
/// so trait defs are import-scoped: `Default`, omitted from the prelude,
/// stays unavailable without `use core::traits::Default`.
#[must_use]
pub fn imported_trait_defs(registry: &ModuleRegistry, module_path: &ModulePath) -> Vec<TraitDef> {
    let mut traits = Vec::new();
    let Ok(resolved) = registry.resolve_imports(module_path) else {
        return traits;
    };
    for (name, bindings) in resolved.imports {
        for import in bindings {
            let ResolvedImport::Symbol {
                from_module,
                export_kind: ExportKind::Trait,
                ..
            } = import
            else {
                continue;
            };
            if let Some(info) = registry.get(&from_module) {
                for item in &info.module.items {
                    if let ItemKind::Trait(def) = &item.kind
                        && def.name == name
                    {
                        traits.push(def.clone());
                    }
                }
            }
        }
    }
    traits
}

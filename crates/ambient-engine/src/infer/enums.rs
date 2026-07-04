//! Enum definitions during type inference.
//!
//! Enums are registered per module check (local declarations plus the
//! built-in `Option`/`Result` prelude). Registration powers two things:
//!
//! - **Constructor expressions** — each variant becomes a scheme in the
//!   type env: `Some` is `∀T. (T) -> Option<T>`, `None` is `∀T. Option<T>`.
//! - **Variant patterns** — `Some(x)` in a `match` unifies the scrutinee
//!   with the enum type and types the payload binding.
//!
//! Variant names resolve unqualified; a local enum's variant shadows a
//! prelude variant with the same name (registration order: prelude first).
//! Variant tags are declaration indices, which is what the compiler and VM
//! use (`Option`: None = 0, Some = 1; `Result`: Ok = 0, Err = 1).

use std::collections::HashMap;
use std::sync::Arc;

use uuid::Uuid;

use crate::types::{NamedType, OPTION_UUID, RESULT_UUID, Type, TypeVarId};

use super::env::Scheme;

/// One variant of a registered enum.
#[derive(Debug, Clone)]
pub struct EnumVariantInfo {
    pub name: Arc<str>,
    /// Payload type, written in terms of the enum's type parameters
    /// (`Named("T")` placeholders).
    pub payload: Option<Type>,
}

/// A registered enum definition.
#[derive(Debug, Clone)]
pub struct EnumInfo {
    pub name: Arc<str>,
    pub type_params: Vec<Arc<str>>,
    /// Variants in declaration order; the index is the runtime tag.
    pub variants: Vec<EnumVariantInfo>,
    /// Nominal identity. Every enum carries `Some(uuid)`: declared enums take
    /// it from their mandatory `unique(<uuid>)` prefix, and the reserved-name
    /// prelude enums (`Option`, `Result`) take the fixed
    /// [`OPTION_UUID`]/[`RESULT_UUID`]. So two structurally identical enums are
    /// always distinct types. `None` only appears transiently, before an
    /// annotation or payload is resolved against the registry.
    pub uuid: Option<Uuid>,
}

/// One variant in a reserved prelude enum's canonical layout.
struct PreludeVariant {
    name: &'static str,
    /// The type parameter this variant's payload is, or `None` for a unit
    /// variant (`Some(T)` → `Some("T")`; `None` → `None`).
    payload_param: Option<&'static str>,
}

/// A reserved-name prelude enum (`Option`/`Result`).
///
/// This is the single source of truth for their nominal uuid, type
/// parameters, and variant layout. Both the type registry
/// ([`EnumRegistry::with_prelude`]) and the compiler's constructor table
/// ([`crate::compiler`]) derive from it, so their tags and payload shapes can
/// never drift apart. Variant declaration order is the runtime tag, which
/// must match how the VM constructs these values (`None`/`Ok` = 0,
/// `Some`/`Err` = 1).
pub(crate) struct PreludeEnum {
    pub name: &'static str,
    pub uuid: Uuid,
    type_params: &'static [&'static str],
    variants: &'static [PreludeVariant],
}

/// The reserved prelude enums, in the order they are registered.
pub(crate) const PRELUDE_ENUMS: &[PreludeEnum] = &[
    PreludeEnum {
        name: "Option",
        uuid: OPTION_UUID,
        type_params: &["T"],
        variants: &[
            PreludeVariant {
                name: "None",
                payload_param: None,
            },
            PreludeVariant {
                name: "Some",
                payload_param: Some("T"),
            },
        ],
    },
    PreludeEnum {
        name: "Result",
        uuid: RESULT_UUID,
        type_params: &["T", "E"],
        variants: &[
            PreludeVariant {
                name: "Ok",
                payload_param: Some("T"),
            },
            PreludeVariant {
                name: "Err",
                payload_param: Some("E"),
            },
        ],
    },
];

impl PreludeEnum {
    /// Build the type-checker's [`EnumInfo`] for this prelude enum.
    fn to_enum_info(&self) -> EnumInfo {
        EnumInfo {
            name: Arc::from(self.name),
            type_params: self.type_params.iter().map(|p| Arc::from(*p)).collect(),
            variants: self
                .variants
                .iter()
                .map(|v| EnumVariantInfo {
                    name: Arc::from(v.name),
                    payload: v.payload_param.map(|p| Type::named(p, vec![])),
                })
                .collect(),
            uuid: Some(self.uuid),
        }
    }

    /// The compiler's constructor layout: `(variant_name, tag, has_payload)`
    /// for each variant, in declaration order.
    pub(crate) fn constructors(&self) -> impl Iterator<Item = (&'static str, u16, bool)> + '_ {
        // Zip against a `u16` range so the tag needs no fallible cast; a
        // reserved prelude enum never has more than a handful of variants.
        self.variants
            .iter()
            .zip(0u16..)
            .map(|(v, tag)| (v.name, tag, v.payload_param.is_some()))
    }
}

/// Registry of enums visible to the module being checked.
#[derive(Debug, Default, Clone)]
pub struct EnumRegistry {
    enums: HashMap<Arc<str>, Arc<EnumInfo>>,
    /// Variant name → enum name (later registrations shadow earlier).
    variant_owners: HashMap<Arc<str>, Arc<str>>,
}

impl EnumRegistry {
    /// Create a registry containing the built-in `Option` and `Result`,
    /// derived from the canonical [`PRELUDE_ENUMS`] specs.
    #[must_use]
    pub fn with_prelude() -> Self {
        let mut registry = Self::default();
        for spec in PRELUDE_ENUMS {
            registry.register(Arc::new(spec.to_enum_info()));
        }
        registry
    }

    /// Register an enum. Its variants shadow same-named variants
    /// registered earlier.
    pub fn register(&mut self, info: Arc<EnumInfo>) {
        for variant in &info.variants {
            self.variant_owners
                .insert(Arc::clone(&variant.name), Arc::clone(&info.name));
        }
        self.enums.insert(Arc::clone(&info.name), info);
    }

    /// Register an enum from its AST definition.
    pub fn register_def(&mut self, def: &crate::ast::EnumDef) {
        self.register(Arc::new(EnumInfo::from_def(def)));
    }

    /// Look up an enum by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Arc<EnumInfo>> {
        self.enums.get(name)
    }

    /// Resolve a variant name to its enum and variant index (= tag).
    #[must_use]
    pub fn resolve_variant(&self, variant: &str) -> Option<(Arc<EnumInfo>, usize)> {
        let enum_name = self.variant_owners.get(variant)?;
        let info = self.enums.get(enum_name)?;
        let idx = info
            .variants
            .iter()
            .position(|v| v.name.as_ref() == variant)?;
        Some((Arc::clone(info), idx))
    }

    /// Iterate over all registered enums.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<EnumInfo>> {
        self.enums.values()
    }
}

impl EnumInfo {
    /// Build registry info from an AST enum definition.
    #[must_use]
    pub fn from_def(def: &crate::ast::EnumDef) -> Self {
        Self {
            name: Arc::clone(&def.name),
            type_params: def
                .type_params
                .iter()
                .map(|tp| Arc::clone(&tp.name))
                .collect(),
            variants: def
                .variants
                .iter()
                .map(|v| EnumVariantInfo {
                    name: Arc::clone(&v.name),
                    payload: v.payload.clone(),
                })
                .collect(),
            uuid: Some(def.uuid),
        }
    }

    /// The constructor scheme for a variant: quantified over the enum's
    /// type parameters, `(payload) -> Enum<params>` for payload variants,
    /// `Enum<params>` for unit variants.
    #[must_use]
    pub fn constructor_scheme(
        &self,
        r#gen: &mut crate::types::TypeVarGen,
        variant_idx: usize,
    ) -> Scheme {
        let mut type_var_map: HashMap<Arc<str>, TypeVarId> = HashMap::new();
        let mut quantified: Vec<TypeVarId> = Vec::new();
        let mut args: Vec<Type> = Vec::new();
        for param in &self.type_params {
            // Quantified ids come from the shared generator so they can
            // never collide with inference variables bound in the global
            // substitution.
            let var_id = r#gen.fresh_id();
            type_var_map.insert(Arc::clone(param), var_id);
            quantified.push(var_id);
            args.push(Type::Var(var_id));
        }

        let enum_ty = Type::Named(NamedType::with_identity(
            Arc::clone(&self.name),
            args,
            self.uuid,
        ));
        let ty = match &self.variants[variant_idx].payload {
            Some(payload) => {
                let payload_ty = super::check::substitute_type_params(payload, &type_var_map);
                Type::function(vec![payload_ty], enum_ty)
            }
            None => enum_ty,
        };

        if quantified.is_empty() {
            Scheme::mono(ty)
        } else {
            Scheme::poly(quantified, ty)
        }
    }

    /// Instantiate the payload type of a variant against fresh type
    /// variables, returning `(enum type, payload type if any)`.
    pub fn instantiate_variant(
        &self,
        infer: &mut super::Infer,
        variant_idx: usize,
    ) -> (Type, Option<Type>) {
        let mut type_var_map: HashMap<Arc<str>, TypeVarId> = HashMap::new();
        let mut args: Vec<Type> = Vec::new();
        for param in &self.type_params {
            let fresh = infer.fresh();
            if let Type::Var(id) = &fresh {
                type_var_map.insert(Arc::clone(param), *id);
            }
            args.push(fresh);
        }

        let enum_ty = Type::Named(NamedType::with_identity(
            Arc::clone(&self.name),
            args,
            self.uuid,
        ));
        // A payload written as another enum's name (e.g. `Wrap(Inner)`)
        // arrives from lowering without that enum's uuid. Resolve it so the
        // binding a pattern extracts carries the payload enum's identity —
        // otherwise a method call on it would key on the head name and miss
        // the uuid-registered impl. `resolve_holes` attaches enum uuids and
        // leaves the already-substituted type variables untouched.
        let payload_ty = self.variants[variant_idx]
            .payload
            .as_ref()
            .map(|p| infer.resolve_holes(&super::check::substitute_type_params(p, &type_var_map)));
        (enum_ty, payload_ty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prelude_option_result_are_nominal() {
        let registry = EnumRegistry::with_prelude();
        let option = registry.get("Option").expect("Option registered");
        let result = registry.get("Result").expect("Result registered");
        assert_eq!(option.uuid, Some(OPTION_UUID));
        assert_eq!(result.uuid, Some(RESULT_UUID));
        assert_ne!(OPTION_UUID, RESULT_UUID);
    }

    #[test]
    fn prelude_variant_tags_match_runtime_layout() {
        // The VM constructs these with fixed tags (`None`/`Ok` = 0,
        // `Some`/`Err` = 1); the registry and the compiler's constructor table
        // both derive from `PRELUDE_ENUMS`, so pin that order here.
        let registry = EnumRegistry::with_prelude();
        let option = registry.get("Option").expect("Option registered");
        assert_eq!(option.variants[0].name.as_ref(), "None");
        assert!(option.variants[0].payload.is_none());
        assert_eq!(option.variants[1].name.as_ref(), "Some");
        assert!(option.variants[1].payload.is_some());

        let result = registry.get("Result").expect("Result registered");
        assert_eq!(result.variants[0].name.as_ref(), "Ok");
        assert_eq!(result.variants[1].name.as_ref(), "Err");

        // The compiler's constructor view must agree with the registry.
        for spec in PRELUDE_ENUMS {
            let info = registry.get(spec.name).expect("spec registered");
            for (variant, tag, has_payload) in spec.constructors() {
                assert_eq!(info.variants[tag as usize].name.as_ref(), variant);
                assert_eq!(info.variants[tag as usize].payload.is_some(), has_payload);
            }
        }
    }
}

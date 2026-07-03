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

use crate::types::{NamedType, Type, TypeVarId};

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
    /// Nominal identity. Declared enums carry `Some(uuid)` from their
    /// mandatory `unique(<uuid>)` prefix, so two structurally identical enums
    /// are distinct types. The reserved-name prelude enums (`Option`,
    /// `Result`) carry `None`: their identity is their reserved head name,
    /// which no user type can claim.
    pub uuid: Option<Uuid>,
}

/// Registry of enums visible to the module being checked.
#[derive(Debug, Default, Clone)]
pub struct EnumRegistry {
    enums: HashMap<Arc<str>, Arc<EnumInfo>>,
    /// Variant name → enum name (later registrations shadow earlier).
    variant_owners: HashMap<Arc<str>, Arc<str>>,
}

impl EnumRegistry {
    /// Create a registry containing the built-in `Option` and `Result`.
    #[must_use]
    pub fn with_prelude() -> Self {
        let mut registry = Self::default();
        registry.register(Arc::new(EnumInfo {
            name: Arc::from("Option"),
            type_params: vec![Arc::from("T")],
            uuid: None,
            variants: vec![
                EnumVariantInfo {
                    name: Arc::from("None"),
                    payload: None,
                },
                EnumVariantInfo {
                    name: Arc::from("Some"),
                    payload: Some(Type::named("T", vec![])),
                },
            ],
        }));
        registry.register(Arc::new(EnumInfo {
            name: Arc::from("Result"),
            type_params: vec![Arc::from("T"), Arc::from("E")],
            uuid: None,
            variants: vec![
                EnumVariantInfo {
                    name: Arc::from("Ok"),
                    payload: Some(Type::named("T", vec![])),
                },
                EnumVariantInfo {
                    name: Arc::from("Err"),
                    payload: Some(Type::named("E", vec![])),
                },
            ],
        }));
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
        let payload_ty = self.variants[variant_idx]
            .payload
            .as_ref()
            .map(|p| super::check::substitute_type_params(p, &type_var_map));
        (enum_ty, payload_ty)
    }
}

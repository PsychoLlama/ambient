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

use crate::fqn::ModuleId;
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
    /// Nominal identity. Every enum carries `Some(uuid)` from its mandatory
    /// `unique(<uuid>)` prefix — including `Option`/`Result`, whose core
    /// declarations claim the fixed [`OPTION_UUID`]/[`RESULT_UUID`] (pinned by
    /// [`validate_reserved_declaration`]). So two structurally identical enums
    /// are always distinct types. `None` only appears transiently, before an
    /// annotation or payload is resolved against the registry.
    pub uuid: Option<Uuid>,
    /// The module that declares this enum — the first two ident segments of
    /// each variant's `Fqn(module, [Enum, Variant])` key. `None` only for
    /// registry-less checks where the resolve pass never ran (`Option`/`Result`
    /// arrive through the prelude carrying their `core::Option`/`core::Result`
    /// module like any other imported enum).
    pub module: Option<ModuleId>,
}

/// One variant in a reserved enum's canonical layout (validation only).
struct ReservedVariant {
    name: &'static str,
    /// The type parameter this variant's payload is, or `None` for a unit
    /// variant (`Some(T)` → `Some("T")`; `None` → `None`).
    payload_param: Option<&'static str>,
}

/// A reserved-identity enum (`Option`/`Result`): its fixed uuid, type
/// parameters, and canonical variant layout.
///
/// This table is *validation-only* — nothing seeds a registry or a
/// constructor table from it. The canonical `Option`/`Result` declarations
/// live in Ambient source (`core_lib/Option.ab`, `core_lib/Result.ab`) and
/// reach the checker and compiler through the module system like any other
/// enum. [`validate_reserved_declaration`] uses this table to guarantee a
/// declaration claiming a reserved uuid *is* the canonical one (same name,
/// type params, variant order), so the core sources can never drift from the
/// spec and no other module can hijack a reserved identity for a different
/// shape. Variant declaration order is the runtime tag (`None`/`Ok` = 0,
/// `Some`/`Err` = 1); the uuids are anchored in `types.rs`
/// ([`OPTION_UUID`]/[`RESULT_UUID`]). This mirrors the shape
/// [`validate_reserved_struct`] uses for the reserved primitives.
struct ReservedEnum {
    name: &'static str,
    uuid: Uuid,
    type_params: &'static [&'static str],
    variants: &'static [ReservedVariant],
}

/// The reserved-identity enums, in canonical variant order.
const RESERVED_ENUMS: &[ReservedEnum] = &[
    ReservedEnum {
        name: "Option",
        uuid: OPTION_UUID,
        type_params: &["T"],
        variants: &[
            ReservedVariant {
                name: "None",
                payload_param: None,
            },
            ReservedVariant {
                name: "Some",
                payload_param: Some("T"),
            },
        ],
    },
    ReservedEnum {
        name: "Result",
        uuid: RESULT_UUID,
        type_params: &["T", "E"],
        variants: &[
            ReservedVariant {
                name: "Ok",
                payload_param: Some("T"),
            },
            ReservedVariant {
                name: "Err",
                payload_param: Some("E"),
            },
        ],
    },
];

/// Validate a source declaration against the reserved-enum specs.
///
/// The canonical `Option`/`Result` declarations live in Ambient source
/// (`core_lib/Option.ab`, `core_lib/Result.ab`) and reach the checker and
/// compiler through the module system. This check pins them to their fixed
/// identity: a declaration that claims a reserved uuid must *be* the canonical
/// declaration — same name, type parameters, and variant layout. The core
/// sources therefore can never drift from the spec (they fail every build if
/// they try), and no other module can hijack a reserved identity for a
/// different shape.
///
/// # Errors
///
/// Returns a description of the mismatch if `def` reuses a reserved uuid
/// without matching its spec. A declaration with a non-reserved uuid
/// always passes.
pub fn validate_reserved_declaration(def: &crate::ast::EnumDef) -> Result<(), String> {
    let Some(spec) = RESERVED_ENUMS.iter().find(|s| s.uuid == def.uuid) else {
        return Ok(());
    };

    let mismatch = |what: &str| {
        Err(format!(
            "`unique({})` is the reserved identity of the prelude enum `{}`; \
             a declaration using it must match the canonical layout exactly ({what})",
            crate::types::uuid_to_source(&spec.uuid),
            spec.name,
        ))
    };

    if def.name.as_ref() != spec.name {
        return mismatch(&format!(
            "expected name `{}`, found `{}`",
            spec.name, def.name
        ));
    }
    let def_params: Vec<&str> = def.type_params.iter().map(|p| p.name.as_ref()).collect();
    if def_params != spec.type_params {
        return mismatch(&format!(
            "expected type parameters {:?}, found {def_params:?}",
            spec.type_params
        ));
    }
    if def.variants.len() != spec.variants.len() {
        return mismatch(&format!(
            "expected {} variants, found {}",
            spec.variants.len(),
            def.variants.len()
        ));
    }
    for (variant, spec_variant) in def.variants.iter().zip(spec.variants) {
        if variant.name.as_ref() != spec_variant.name {
            return mismatch(&format!(
                "expected variant `{}`, found `{}`",
                spec_variant.name, variant.name
            ));
        }
        let payload_matches = match (&variant.payload, spec_variant.payload_param) {
            (None, None) => true,
            (Some(Type::Named(named)), Some(param)) => {
                named.name.as_ref() == param && named.args.is_empty()
            }
            // A reserved enum's payload is a raw AST `Named`, never a rigid
            // `Param` (validation runs at registration, outside any body
            // scope); this arm keeps the comparison correct if that ever
            // changes.
            (Some(Type::Param(name)), Some(param)) => name.as_ref() == param,
            _ => false,
        };
        if !payload_matches {
            return mismatch(&format!(
                "variant `{}` must carry {}",
                spec_variant.name,
                spec_variant
                    .payload_param
                    .map_or_else(|| "no payload".to_string(), |p| format!("payload `{p}`")),
            ));
        }
    }
    Ok(())
}

/// Validate a struct declaration against the reserved primitive specs.
///
/// The canonical `Bool`/`Number`/`String`/`Binary` declarations live in Ambient
/// source (`core_lib/bool.ab`, ...) as `extern` unit structs, while the
/// compiler anchors their identity on the reserved uuids in `types.rs`
/// ([`crate::types::Primitive`]). This check pins the two together: a
/// declaration that claims a reserved uuid must *be* the canonical declaration
/// — same name, `extern`, unit shape, and no type parameters — and a
/// declaration claiming a reserved *name* must carry the matching uuid. The
/// core sources can therefore never drift from the anchors, and no other module
/// can hijack a primitive identity.
///
/// # Errors
///
/// Returns a description of the mismatch if `def` reuses a reserved primitive
/// uuid without matching its spec, or claims a reserved primitive name without
/// the matching uuid. A declaration touching neither always passes.
pub fn validate_reserved_struct(def: &crate::ast::StructDef) -> Result<(), String> {
    use crate::types::Primitive;

    let by_uuid = def.unique_id.and_then(Primitive::from_uuid);
    let by_name = Primitive::from_name(&def.name);

    let Some(prim) = by_uuid else {
        // A struct claiming a reserved primitive *name* without the reserved
        // uuid is an attempted identity hijack.
        if let Some(prim) = by_name {
            return Err(format!(
                "`{}` is the reserved built-in primitive; a struct named `{}` must use its \
                 reserved identity `extern unique({}) struct {};`",
                prim.name(),
                prim.name(),
                crate::types::uuid_to_source(&prim.uuid()),
                prim.name(),
            ));
        }
        return Ok(());
    };

    let mismatch = |what: &str| {
        Err(format!(
            "`unique({})` is the reserved identity of the built-in primitive `{}`; a \
             declaration using it must be `extern unique(...) struct {};` ({what})",
            crate::types::uuid_to_source(&prim.uuid()),
            prim.name(),
            prim.name(),
        ))
    };

    if def.name.as_ref() != prim.name() {
        return mismatch(&format!(
            "expected name `{}`, found `{}`",
            prim.name(),
            def.name
        ));
    }
    if !def.is_extern {
        return mismatch("must be declared `extern`");
    }
    if !def.is_unit() {
        return mismatch("must be a unit struct carrying no fields");
    }
    if !def.type_params.is_empty() {
        return mismatch("must have no type parameters");
    }
    Ok(())
}

/// Registry of enums visible to the module being checked.
#[derive(Debug, Default, Clone)]
pub struct EnumRegistry {
    enums: HashMap<Arc<str>, Arc<EnumInfo>>,
    /// Variant name → enum name (later registrations shadow earlier).
    variant_owners: HashMap<Arc<str>, Arc<str>>,
}

impl EnumRegistry {
    /// Register an enum. Its variants shadow same-named variants
    /// registered earlier.
    pub fn register(&mut self, info: Arc<EnumInfo>) {
        for variant in &info.variants {
            self.variant_owners
                .insert(Arc::clone(&variant.name), Arc::clone(&info.name));
        }
        self.enums.insert(Arc::clone(&info.name), info);
    }

    /// Register an enum from its AST definition, tagged with its declaring
    /// module (`None` for prelude/registry-less registration).
    pub fn register_def(&mut self, def: &crate::ast::EnumDef, module: Option<ModuleId>) {
        self.register(Arc::new(EnumInfo::from_def(def, module)));
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
    /// Build registry info from an AST enum definition, tagged with its
    /// declaring module.
    #[must_use]
    pub fn from_def(def: &crate::ast::EnumDef, module: Option<ModuleId>) -> Self {
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
            module,
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
    use crate::types::{Primitive, STRING_UUID};

    #[test]
    fn reserved_enums_have_fixed_identity() {
        // Option/Result no longer seed any registry; they flow in from the
        // core `.ab` sources through the module system. The validation-only
        // spec still anchors their reserved uuids (end-to-end construction is
        // covered by `ambient-cli`'s `core_prelude_enums` tests).
        let option = RESERVED_ENUMS.iter().find(|e| e.name == "Option").unwrap();
        let result = RESERVED_ENUMS.iter().find(|e| e.name == "Result").unwrap();
        assert_eq!(option.uuid, OPTION_UUID);
        assert_eq!(result.uuid, RESULT_UUID);
        assert_ne!(OPTION_UUID, RESULT_UUID);
    }

    #[test]
    fn reserved_enum_variant_order_matches_runtime_layout() {
        // The VM constructs these with fixed tags (`None`/`Ok` = 0,
        // `Some`/`Err` = 1). `validate_reserved_declaration` pins the core
        // `.ab` declarations to this order, so a canonical declaration's
        // runtime tags can never drift from the VM.
        let option = RESERVED_ENUMS.iter().find(|e| e.name == "Option").unwrap();
        assert_eq!(option.variants[0].name, "None");
        assert!(option.variants[0].payload_param.is_none());
        assert_eq!(option.variants[1].name, "Some");
        assert!(option.variants[1].payload_param.is_some());

        let result = RESERVED_ENUMS.iter().find(|e| e.name == "Result").unwrap();
        assert_eq!(result.variants[0].name, "Ok");
        assert_eq!(result.variants[1].name, "Err");
    }

    fn enum_def(
        name: &str,
        uuid: Uuid,
        type_params: &[&str],
        variants: &[(&str, Option<&str>)],
    ) -> crate::ast::EnumDef {
        crate::ast::EnumDef {
            name: Arc::from(name),
            name_span: crate::ast::Span::default(),
            is_public: true,
            type_params: type_params
                .iter()
                .map(|p| crate::ast::TypeParam {
                    name: Arc::from(*p),
                    span: crate::ast::Span::default(),
                })
                .collect(),
            variants: variants
                .iter()
                .map(|(v, payload)| crate::ast::EnumVariant {
                    name: Arc::from(*v),
                    payload: payload.map(|p| Type::named(p, vec![])),
                    span: crate::ast::Span::default(),
                })
                .collect(),
            uuid,
        }
    }

    #[test]
    fn reserved_uuid_accepts_the_canonical_declaration() {
        let option = enum_def(
            "Option",
            OPTION_UUID,
            &["T"],
            &[("None", None), ("Some", Some("T"))],
        );
        assert!(validate_reserved_declaration(&option).is_ok());

        let result = enum_def(
            "Result",
            RESULT_UUID,
            &["T", "E"],
            &[("Ok", Some("T")), ("Err", Some("E"))],
        );
        assert!(validate_reserved_declaration(&result).is_ok());
    }

    #[test]
    fn reserved_uuid_rejects_any_other_shape() {
        // Wrong name: a hijack of Option's identity.
        let hijack = enum_def(
            "MyOption",
            OPTION_UUID,
            &["T"],
            &[("None", None), ("Some", Some("T"))],
        );
        assert!(validate_reserved_declaration(&hijack).is_err());

        // Wrong variant order: would flip the runtime tags.
        let flipped = enum_def(
            "Option",
            OPTION_UUID,
            &["T"],
            &[("Some", Some("T")), ("None", None)],
        );
        assert!(validate_reserved_declaration(&flipped).is_err());

        // Wrong payload.
        let payloadless = enum_def(
            "Result",
            RESULT_UUID,
            &["T", "E"],
            &[("Ok", Some("T")), ("Err", None)],
        );
        assert!(validate_reserved_declaration(&payloadless).is_err());

        // A non-reserved uuid is free to be anything.
        let user = enum_def(
            "MyOption",
            Uuid::from_u128(0x1234),
            &["T"],
            &[("Some", Some("T")), ("None", None)],
        );
        assert!(validate_reserved_declaration(&user).is_ok());
    }

    /// Build a `StructDef` the way the parser lowers a `struct` declaration:
    /// `unique(uuid)` wraps the (possibly empty) record body in `Type::Nominal`.
    fn struct_def(
        name: &str,
        uuid: Option<Uuid>,
        is_extern: bool,
        type_params: &[&str],
        fields: &[(&str, Type)],
    ) -> crate::ast::StructDef {
        let record = Type::Record(crate::types::RecordType {
            fields: fields
                .iter()
                .map(|(n, t)| (Arc::from(*n), t.clone()))
                .collect(),
        });
        let ty = match uuid {
            Some(uuid) => Type::Nominal(
                crate::types::NominalType::new(uuid, record, Some(name)).with_extern(is_extern),
            ),
            None => record,
        };
        crate::ast::StructDef {
            name: Arc::from(name),
            name_span: crate::ast::Span::default(),
            is_public: true,
            type_params: type_params
                .iter()
                .map(|p| crate::ast::TypeParam {
                    name: Arc::from(*p),
                    span: crate::ast::Span::default(),
                })
                .collect(),
            ty,
            unique_id: uuid,
            is_extern,
        }
    }

    #[test]
    fn reserved_struct_accepts_the_canonical_extern_declaration() {
        for prim in [
            Primitive::Bool,
            Primitive::Number,
            Primitive::String,
            Primitive::Binary,
        ] {
            let def = struct_def(prim.name(), Some(prim.uuid()), true, &[], &[]);
            assert!(
                validate_reserved_struct(&def).is_ok(),
                "canonical `{}` must validate",
                prim.name()
            );
        }
    }

    #[test]
    fn reserved_struct_rejects_drift_and_hijacks() {
        // Reserved uuid but not `extern`: a constructable primitive is a footgun.
        let not_extern = struct_def("String", Some(STRING_UUID), false, &[], &[]);
        assert!(validate_reserved_struct(&not_extern).is_err());

        // Reserved uuid, wrong name.
        let wrong_name = struct_def("Str", Some(STRING_UUID), true, &[], &[]);
        assert!(validate_reserved_struct(&wrong_name).is_err());

        // Reserved uuid, carries fields (not a unit struct).
        let with_fields = struct_def(
            "String",
            Some(STRING_UUID),
            true,
            &[],
            &[("v", Type::number())],
        );
        assert!(validate_reserved_struct(&with_fields).is_err());

        // Reserved uuid, spurious type parameter.
        let generic = struct_def("String", Some(STRING_UUID), true, &["T"], &[]);
        assert!(validate_reserved_struct(&generic).is_err());

        // Reserved *name* without the reserved uuid: an identity hijack.
        let hijack = struct_def("String", Some(Uuid::from_u128(0x1234)), true, &[], &[]);
        assert!(validate_reserved_struct(&hijack).is_err());

        // A struct touching neither a reserved name nor uuid is free.
        let user = struct_def("Widget", Some(Uuid::from_u128(0x1234)), false, &[], &[]);
        assert!(validate_reserved_struct(&user).is_ok());
    }
}

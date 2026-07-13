//! Module interfaces: the deterministic, content-keyed description of
//! everything a *dependent module* can observe about a module.
//!
//! This is the interface half of the incremental-compilation cache key
//! (Phase 1). A module's check/compile output is a function of its own
//! resolved source, the `pub` interfaces of the modules it depends on, the
//! build-global dispatch/ability surface, and its linked callee hashes.
//! [`ModuleInterface`] captures the cross-module-observable surface; the
//! per-module resolved-AST hash ([`ast_hash::module_ast_hash`]) captures the
//! "own source" half; the package [`dispatch_surface_hash`] captures the
//! build-global coherence channel.
//!
//! # Why parse + resolve is enough
//!
//! `pub` items require full type annotations (see `ref/architecture.md`,
//! "Type Inference Rules"), so the exported surface is derivable from the
//! resolved AST alone — no type inference. Every field here is read off the
//! post-resolve module AST and the registry; nothing waits on the checker.
//! Nominal identities come straight from the declaration uuids (`enum`,
//! `struct`, `ability`, `trait`), value-object hashes reuse the existing
//! name-independent const hashing, and method bodies fold in as span-free
//! structural AST hashes.
//!
//! # Determinism
//!
//! Every collection is sorted (impls by a derived key, everything else by
//! name), no field carries an interner index or an inference variable id,
//! and the encoding is canonical (`encode.rs`): `decode ∘ encode` is the
//! identity and [`ModuleInterface::interface_hash`] is stable across builds.

mod ast_hash;
mod encode;
mod structured;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;

pub use ast_hash::{module_ast_hash, render_type};
pub use encode::InterfaceError;
pub use structured::{ItemKindTag, ItemNamespace, StructuredItem, structured_items};

use crate::ast::ItemKind;
use crate::fqn::{ModuleId, Scope};
use crate::module_path::ModulePath;
use crate::module_registry::{ExportKind, ModuleRegistry, ReExport};
use crate::types::Type;

/// The observable surface of one module.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModuleInterface {
    /// The module's identity, rendered (`core::option`,
    /// `workspace::pkg::utils`). Informational; the canonical key is the
    /// [`ModuleId`] on [`ModuleInterfaceSummary`].
    pub module: String,
    /// `pub` function and `extern fn` signatures, sorted by name.
    pub functions: Vec<FnSig>,
    /// `pub` const declarations, sorted by name.
    pub consts: Vec<ConstEntry>,
    /// `pub` struct shapes, sorted by name.
    pub structs: Vec<StructShape>,
    /// `pub` enum shapes (variant/tag order preserved), sorted by name.
    pub enums: Vec<EnumShape>,
    /// `pub` type aliases, sorted by name.
    pub aliases: Vec<AliasShape>,
    /// `pub` trait definitions, sorted by name.
    pub traits: Vec<TraitShape>,
    /// `pub` ability definitions, sorted by name.
    pub abilities: Vec<AbilityShape>,
    /// Every impl (trait and inherent) in the module — observable build-wide
    /// through coherence and dispatch — sorted by a derived key.
    pub impls: Vec<ImplShape>,
    /// `pub use` re-exports resolved to their target, sorted by local name.
    pub reexports: Vec<ReExportEntry>,
    /// `pub extern fn` native identities `(uuid, arity)`, sorted by name.
    pub externs: Vec<ExternEntry>,
}

/// A `pub` function or `extern fn` signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnSig {
    /// Item name.
    pub name: String,
    /// Rendered parameter types, in order.
    pub params: Vec<String>,
    /// Rendered return type.
    pub ret: String,
    /// Rendered ability (`with`) row, sorted.
    pub abilities: Vec<String>,
}

/// A `pub` const declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstEntry {
    /// Const name.
    pub name: String,
    /// Rendered declared/inferred type.
    pub ty: String,
    /// The content hash of the const's value object (name-independent, the
    /// same hash the defining module ships), or `None` if the initializer
    /// is not a content-addressable literal.
    pub value_hash: Option<[u8; 32]>,
}

/// A `pub` struct shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructShape {
    /// Struct name.
    pub name: String,
    /// Nominal uuid (`unique(...)`), rendered; empty for a structural struct.
    pub uuid: String,
    /// Whether this is an `extern` (engine-provided) struct.
    pub is_extern: bool,
    /// Generic parameter names, in order.
    pub type_params: Vec<String>,
    /// Fields as `(name, rendered type)`, sorted by name.
    pub fields: Vec<(String, String)>,
}

/// A `pub` enum shape. Variant order is the tag order dependents inline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumShape {
    /// Enum name.
    pub name: String,
    /// Nominal uuid (`unique(...)`), rendered.
    pub uuid: String,
    /// Generic parameter names, in order.
    pub type_params: Vec<String>,
    /// Variants as `(name, rendered payload type or empty)`, in tag order.
    pub variants: Vec<(String, Option<String>)>,
}

/// A `pub` type alias.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasShape {
    /// Alias name.
    pub name: String,
    /// Generic parameter names, in order.
    pub type_params: Vec<String>,
    /// Rendered aliased type.
    pub target: String,
}

/// A `pub` trait definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitShape {
    /// Trait name.
    pub name: String,
    /// Nominal uuid (`unique(...)`), rendered.
    pub uuid: String,
    /// Generic parameter names, in order.
    pub type_params: Vec<String>,
    /// Rendered supertrait references, sorted.
    pub supertraits: Vec<String>,
    /// Method signatures, sorted by name.
    pub methods: Vec<TraitMethodSig>,
}

/// A trait method signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitMethodSig {
    /// Method name.
    pub name: String,
    /// Whether the method takes `self`.
    pub has_self: bool,
    /// Rendered parameter types (excluding `self`), in order.
    pub params: Vec<String>,
    /// Rendered return type.
    pub ret: String,
    /// Rendered ability row, sorted.
    pub abilities: Vec<String>,
}

/// A `pub` ability definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbilityShape {
    /// Ability name.
    pub name: String,
    /// The content-addressed [`AbilityId`](ambient_core::AbilityId) bytes,
    /// derived from the declaration uuid.
    pub ability_id: [u8; 32],
    /// Rendered dependency (`with`) references, sorted.
    pub dependencies: Vec<String>,
    /// Methods in declaration order.
    pub methods: Vec<AbilityMethodEntry>,
}

/// One ability method's observable surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbilityMethodEntry {
    /// Method name.
    pub name: String,
    /// Rendered parameter types, in order.
    pub params: Vec<String>,
    /// Rendered return type.
    pub ret: String,
    /// Whether the method returns `!` (never) — an unhandled perform
    /// unwinds rather than capturing a continuation.
    pub never: bool,
    /// Span-free AST content hash of the default implementation body, or
    /// `None` for an abstract never-returning method (no body). A
    /// dependent's `MethodKey` folds in the compiled default-impl hash, so
    /// a body edit must move this.
    pub body_hash: Option<[u8; 32]>,
}

/// An impl block (trait or inherent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplShape {
    /// Rendered trait reference (resolved `Fqn`), or `None` for an inherent
    /// impl.
    pub trait_ref: Option<String>,
    /// Rendered implementing type.
    pub for_type: String,
    /// Generic parameter names, in order.
    pub type_params: Vec<String>,
    /// Method implementations, sorted by name.
    pub methods: Vec<ImplMethodEntry>,
}

impl ImplShape {
    /// A stable sort key so the impl list order never depends on source
    /// order. Two impls that collided here would be a coherence error the
    /// checker reports anyway.
    fn sort_key(&self) -> String {
        let methods: Vec<&str> = self.methods.iter().map(|m| m.name.as_str()).collect();
        format!(
            "{}|{}|{}",
            self.trait_ref.as_deref().unwrap_or(""),
            self.for_type,
            methods.join(",")
        )
    }
}

/// One impl method's observable surface: its signature plus the content
/// hash of its body (dependents link dispatch symbols whose final hashes
/// embed impl bodies).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplMethodEntry {
    /// Method name.
    pub name: String,
    /// Whether the method takes `self`.
    pub has_self: bool,
    /// Rendered parameter types (excluding `self`), in order.
    pub params: Vec<String>,
    /// Rendered return type, or empty if omitted.
    pub ret: String,
    /// Rendered ability row, sorted.
    pub abilities: Vec<String>,
    /// Span-free AST content hash of the method body.
    pub body_hash: [u8; 32],
}

/// A resolved `pub use` re-export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReExportEntry {
    /// The local name this re-export exposes.
    pub local: String,
    /// The kind of re-export target (see [`encode`] kind tags).
    pub kind: u8,
    /// The rendered target: a `Fqn` for an item, a `ModuleId` for a whole
    /// module, or the spelled path if resolution failed.
    pub target: String,
}

/// A `pub extern fn`'s native identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternEntry {
    /// Item name.
    pub name: String,
    /// The host-supplied stable uuid, rendered — the native's content
    /// identity — or `None` when no binding is registered at build time.
    pub uuid: Option<String>,
    /// Declared parameter count.
    pub arity: u8,
}

/// A module interface plus its derived hashes.
#[derive(Debug, Clone)]
pub struct ModuleInterfaceSummary {
    /// The module's canonical identity.
    pub module: ModuleId,
    /// The observable surface.
    pub interface: ModuleInterface,
    /// `blake3` of the canonical interface encoding.
    pub interface_hash: blake3::Hash,
    /// Span-free structural hash of the whole resolved module AST (private
    /// items included) — the "own source" half of the future cache key.
    pub resolved_ast_hash: blake3::Hash,
    /// The module's source file path relative to the package `src/`
    /// directory (`utils/format.ab`), for debug-symbol correlation. Empty
    /// for builtin (`core`/platform) modules, which are embedded and have no
    /// on-disk source.
    pub source_path: String,
    /// The structured, spanned index of every top-level item (private
    /// included), sorted. Additive to [`Self::interface`]: it is *not* folded
    /// into [`Self::interface_hash`], so it never perturbs a cache key.
    pub items: Vec<StructuredItem>,
}

// Re-export target kind tags (also used by `encode`).
pub(crate) const REKIND_UNRESOLVED: u8 = 0;
pub(crate) const REKIND_MODULE: u8 = 1;
pub(crate) const REKIND_FUNCTION: u8 = 2;
pub(crate) const REKIND_CONST: u8 = 3;
pub(crate) const REKIND_STRUCT: u8 = 4;
pub(crate) const REKIND_TYPE_ALIAS: u8 = 5;
pub(crate) const REKIND_ENUM: u8 = 6;
pub(crate) const REKIND_ENUM_VARIANT: u8 = 7;
pub(crate) const REKIND_ABILITY: u8 = 8;
pub(crate) const REKIND_TRAIT: u8 = 9;

const fn export_kind_tag(kind: ExportKind) -> u8 {
    match kind {
        ExportKind::Function => REKIND_FUNCTION,
        ExportKind::Const => REKIND_CONST,
        ExportKind::Struct => REKIND_STRUCT,
        ExportKind::TypeAlias => REKIND_TYPE_ALIAS,
        ExportKind::Enum => REKIND_ENUM,
        ExportKind::EnumVariant => REKIND_ENUM_VARIANT,
        ExportKind::Ability => REKIND_ABILITY,
        ExportKind::Trait => REKIND_TRAIT,
    }
}

impl ModuleInterface {
    /// `blake3` of the canonical byte encoding.
    #[must_use]
    pub fn interface_hash(&self) -> blake3::Hash {
        blake3::hash(&self.encode())
    }

    /// Build the interface of `module_path` from a resolved registry.
    ///
    /// The registry must already have run the resolve pass (so nominal
    /// references carry their identities); every observable channel is read
    /// off the resolved AST and the registry's native/import views.
    #[must_use]
    pub fn from_module(registry: &ModuleRegistry, module_path: &ModulePath) -> Self {
        let module_id = registry.module_id(module_path);
        let Some(info) = registry.get(module_path) else {
            return Self {
                module: module_id.to_string(),
                ..Self::default()
            };
        };
        let natives = registry.natives().keys_for_module(module_path);

        let mut out = Self {
            module: module_id.to_string(),
            ..Self::default()
        };

        for item in &info.module.items {
            match &item.kind {
                ItemKind::Function(f) if f.is_public => out.functions.push(FnSig {
                    name: f.name.to_string(),
                    params: f
                        .params
                        .iter()
                        .map(|p| render_type(p.declared_ty()))
                        .collect(),
                    ret: f
                        .ret_ty
                        .as_ref()
                        .map_or_else(|| "_".to_string(), render_type),
                    abilities: sorted_names(&f.abilities),
                }),
                ItemKind::ExternFn(e) if e.is_public => {
                    out.functions.push(FnSig {
                        name: e.name.to_string(),
                        params: e
                            .params
                            .iter()
                            .map(|p| render_type(p.declared_ty()))
                            .collect(),
                        ret: render_type(&e.ret_ty),
                        abilities: Vec::new(),
                    });
                    let key = natives.get(&e.name);
                    #[allow(clippy::cast_possible_truncation)]
                    out.externs.push(ExternEntry {
                        name: e.name.to_string(),
                        uuid: key.map(|k| k.uuid.to_string()),
                        arity: key.map_or(e.params.len() as u8, |k| k.arity),
                    });
                }
                ItemKind::Const(c) if c.is_public => out.consts.push(ConstEntry {
                    name: c.name.to_string(),
                    ty: c
                        .ty
                        .clone()
                        .or_else(|| crate::const_eval::literal_type(&c.value))
                        .map_or_else(|| "_".to_string(), |t| render_type(&t)),
                    value_hash: const_value_hash(c),
                }),
                ItemKind::Struct(s) if s.is_public => out.structs.push(StructShape {
                    name: s.name.to_string(),
                    uuid: s.unique_id.map(|u| u.to_string()).unwrap_or_default(),
                    is_extern: s.is_extern,
                    type_params: param_names(&s.type_params),
                    fields: record_fields(&s.ty)
                        .into_iter()
                        .map(|(n, t)| (n.to_string(), render_type(&t)))
                        .collect(),
                }),
                ItemKind::Enum(e) if e.is_public => out.enums.push(EnumShape {
                    name: e.name.to_string(),
                    uuid: e.uuid.to_string(),
                    type_params: param_names(&e.type_params),
                    variants: e
                        .variants
                        .iter()
                        .map(|v| (v.name.to_string(), v.payload.as_ref().map(render_type)))
                        .collect(),
                }),
                ItemKind::TypeAlias(t) if t.is_public => out.aliases.push(AliasShape {
                    name: t.name.to_string(),
                    type_params: param_names(&t.type_params),
                    target: render_type(&t.ty),
                }),
                ItemKind::Trait(t) if t.is_public => out.traits.push(trait_shape(t)),
                ItemKind::Ability(a) if a.is_public => out.abilities.push(ability_shape(a)),
                ItemKind::Impl(i) => out.impls.push(impl_shape(i)),
                _ => {}
            }
        }

        out.reexports = reexports(registry, module_path, &info.re_exports);

        out.sort();
        out
    }

    /// Sort every collection into its canonical order.
    fn sort(&mut self) {
        self.functions.sort_by(|a, b| a.name.cmp(&b.name));
        self.consts.sort_by(|a, b| a.name.cmp(&b.name));
        self.structs.sort_by(|a, b| a.name.cmp(&b.name));
        self.enums.sort_by(|a, b| a.name.cmp(&b.name));
        self.aliases.sort_by(|a, b| a.name.cmp(&b.name));
        self.traits.sort_by(|a, b| a.name.cmp(&b.name));
        self.abilities.sort_by(|a, b| a.name.cmp(&b.name));
        self.impls.sort_by_key(ImplShape::sort_key);
        self.reexports.sort_by(|a, b| a.local.cmp(&b.local));
        self.externs.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

fn trait_shape(t: &crate::ast::TraitDef) -> TraitShape {
    TraitShape {
        name: t.name.to_string(),
        uuid: t.uuid.to_string(),
        type_params: param_names(&t.type_params),
        supertraits: sorted_names(&t.supertraits),
        methods: {
            let mut methods: Vec<TraitMethodSig> = t
                .methods
                .iter()
                .map(|m| TraitMethodSig {
                    name: m.name.to_string(),
                    has_self: m.has_self,
                    params: m.params.iter().map(|(_, ty)| render_type(ty)).collect(),
                    ret: render_type(&m.ret_ty),
                    abilities: sorted_names(&m.abilities),
                })
                .collect();
            methods.sort_by(|a, b| a.name.cmp(&b.name));
            methods
        },
    }
}

fn ability_shape(a: &crate::ast::AbilityDef) -> AbilityShape {
    AbilityShape {
        name: a.name.to_string(),
        ability_id: *ambient_core::AbilityId::from_uuid(&a.uuid).as_bytes(),
        dependencies: sorted_names(&a.dependencies),
        methods: a
            .methods
            .iter()
            .map(|m| AbilityMethodEntry {
                name: m.name.to_string(),
                params: m
                    .params
                    .iter()
                    .map(|p| render_type(p.declared_ty()))
                    .collect(),
                ret: render_type(&m.ret_ty),
                never: matches!(&m.ret_ty, Type::Never),
                body_hash: m
                    .body
                    .as_ref()
                    .map(|body| ast_hash::hash_body(&m.params, body)),
            })
            .collect(),
    }
}

fn impl_shape(i: &crate::ast::ImplDef) -> ImplShape {
    let mut methods: Vec<ImplMethodEntry> = i
        .methods
        .iter()
        .map(|m| ImplMethodEntry {
            name: m.name.to_string(),
            has_self: m.has_self,
            params: m
                .params
                .iter()
                .map(|p| render_type(p.declared_ty()))
                .collect(),
            ret: m.ret_ty.as_ref().map_or_else(String::new, render_type),
            abilities: sorted_names(&m.abilities),
            body_hash: ast_hash::hash_body(&m.params, &m.body),
        })
        .collect();
    methods.sort_by(|a, b| a.name.cmp(&b.name));
    ImplShape {
        trait_ref: i.trait_name.as_ref().map(ast_hash::render_name),
        for_type: render_type(&i.for_type),
        type_params: param_names(&i.type_params),
        methods,
    }
}

fn reexports(
    registry: &ModuleRegistry,
    module_path: &ModulePath,
    re_exports: &[ReExport],
) -> Vec<ReExportEntry> {
    let mut out = Vec::new();
    for re in re_exports {
        let Some(local) = re.exported_name() else {
            continue;
        };
        // A symbol re-export resolves through the engine's own re-export
        // chasing (origin module + defining name); a whole-module re-export
        // resolves to the target module path.
        if let Ok((export, defining)) = registry.lookup_symbol(module_path, local) {
            let fqn = registry.fqn(&defining, &[Arc::clone(&export.name)]);
            out.push(ReExportEntry {
                local: local.to_string(),
                kind: export_kind_tag(export.kind),
                target: fqn.to_string(),
            });
        } else if let Ok(target) = registry.resolve_use_path(module_path, &re.prefix, &re.path) {
            out.push(ReExportEntry {
                local: local.to_string(),
                kind: REKIND_MODULE,
                target: registry.module_id(&target).to_string(),
            });
        } else {
            out.push(ReExportEntry {
                local: local.to_string(),
                kind: REKIND_UNRESOLVED,
                target: spelled_use(re),
            });
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Package-level derivations
// ─────────────────────────────────────────────────────────────────────────────

/// Build an interface summary for every module registered in `registry`,
/// keyed by the module's canonical identity string.
///
/// This is the single decision point for "what does a module expose" —
/// shared by [`build_package`](crate::build::build_package) and the analysis
/// pipeline so the compiler and the editor derive identical keys.
#[must_use]
pub fn build_interfaces(registry: &ModuleRegistry) -> BTreeMap<String, ModuleInterfaceSummary> {
    let mut out = BTreeMap::new();
    for info in registry.all_modules() {
        let module = registry.module_id(&info.path);
        let interface = ModuleInterface::from_module(registry, &info.path);
        let interface_hash = interface.interface_hash();
        let resolved_ast_hash = ast_hash::module_ast_hash(&info.module);
        let source_path = module_source_path(&module, info);
        let items = structured_items(&info.module);
        out.insert(
            module.to_string(),
            ModuleInterfaceSummary {
                module,
                interface,
                interface_hash,
                resolved_ast_hash,
                source_path,
                items,
            },
        );
    }
    out
}

/// The module's source path relative to the package `src/` directory, via the
/// canonical file↔module mapping ([`ModulePath::to_file_path`]). Empty for
/// builtin modules (embedded, no on-disk source). A directory module's file
/// is its `main.ab`, so it renders `<dir>/main.ab` rather than `<dir>.ab`.
///
/// Shared by [`build_interfaces`] and the analysis session so both derive an
/// identical summary.
#[must_use]
pub fn module_source_path(module: &ModuleId, info: &crate::module_registry::ModuleInfo) -> String {
    if matches!(module.scope, Scope::Builtin) {
        return String::new();
    }
    // The loader-recorded real path is authoritative: it is the only source
    // that distinguishes a directory module's `<dir>/main.ab` from a
    // file-backed `<dir>.ab` without relying on the `is_dir_module` flag
    // (which the analysis/build registration path leaves `false`).
    if let Some(recorded) = &info.source_path {
        return recorded.clone();
    }
    let segments: Vec<&str> = info.path.segments().iter().map(AsRef::as_ref).collect();
    if info.is_dir_module {
        // A directory module is backed by `<dir>/main.ab`.
        let mut parts = segments;
        parts.push("main");
        format!("{}.ab", parts.join("/"))
    } else if segments.is_empty() {
        // The package-root module (`main.ab`).
        "main.ab".to_string()
    } else {
        format!("{}.ab", segments.join("/"))
    }
}

/// The build-global dispatch-surface hash: a deterministic fold of every
/// module's **body-free** impl and ability sections (the coherence/dispatch
/// channel that no single per-module interface owns).
///
/// Method bodies are excluded (see [`ModuleInterface::dispatch_bytes`]): a
/// body edit no longer moves this hash, so it no longer invalidates every
/// module's cache key. What remains is the coherence/dispatch *shape* —
/// `(trait, type)` impl pairs and dispatched signatures — which genuinely is
/// build-global (a duplicate impl anywhere is an error in every module, and a
/// signature change alters every dispatcher's inference). Body-driven object
/// changes are covered elsewhere: link validation for the build cache, the
/// dependency `interface_hash` (which retains bodies) for importers.
#[must_use]
pub fn dispatch_surface_hash(
    interfaces: &BTreeMap<String, ModuleInterfaceSummary>,
) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"ambient/interface/dispatch/v2");
    #[allow(clippy::cast_possible_truncation)]
    for (key, summary) in interfaces {
        let bytes = summary.interface.dispatch_bytes();
        hasher.update(&(key.len() as u32).to_le_bytes());
        hasher.update(key.as_bytes());
        hasher.update(&(bytes.len() as u32).to_le_bytes());
        hasher.update(&bytes);
    }
    hasher.finalize()
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn param_names(tps: &[crate::ast::TypeParam]) -> Vec<String> {
    tps.iter().map(|tp| tp.name.to_string()).collect()
}

fn sorted_names(names: &[crate::ast::QualifiedName]) -> Vec<String> {
    let mut out: Vec<String> = names.iter().map(ast_hash::render_name).collect();
    out.sort_unstable();
    out
}

fn record_fields(ty: &Type) -> Vec<(Arc<str>, Type)> {
    match ty {
        Type::Nominal(n) => record_fields(&n.inner),
        Type::Record(r) => r.fields.clone(),
        _ => Vec::new(),
    }
}

fn const_value_hash(c: &crate::ast::ConstDef) -> Option<[u8; 32]> {
    let value = crate::const_eval::literal_value(&c.value)?;
    let object = crate::object::value_object(&value).ok()?;
    Some(*object.hash().as_bytes())
}

fn spelled_use(re: &ReExport) -> String {
    let prefix = match re.prefix {
        crate::ast::UsePrefix::Pkg => "pkg",
        crate::ast::UsePrefix::Core => "core",
        crate::ast::UsePrefix::Self_ => "self",
        crate::ast::UsePrefix::Super(_) => "super",
        crate::ast::UsePrefix::Local => "local",
    };
    let path: Vec<&str> = re.path.iter().map(AsRef::as_ref).collect();
    format!("{prefix}::{}", path.join("::"))
}

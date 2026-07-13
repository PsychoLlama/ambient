//! A structured, spanned index of a module's top-level items — the debug
//! symbol / codebase-intelligence half of a build snapshot.
//!
//! The [`ModuleInterface`](super::ModuleInterface) records the *cross-module
//! observable* surface (public items, rendered to strings, span-free, for
//! content-keyed cache invalidation). This index is the complementary view:
//! **every** top-level item (private included), each namespace-tagged and
//! carrying its definition span, so a `store show pkg::shapes::Shape` can
//! locate a type, trait, or ability that is not an object and correlate a
//! hash back to a source location.
//!
//! Spans are byte offsets into the module's source. Everything here is read
//! straight off the resolved AST at interface-build time (where spans still
//! exist), then folded into the manifest by
//! [`BuildManifest::from_build`](crate::disk_store::BuildManifest::from_build),
//! which fills in the object hash for value items from the build's name
//! bindings.

use std::sync::Arc;

use crate::ast::{ItemKind, Module, Param};
use crate::module_interface::render_type;

/// The namespace an item lives in — the coarse tag the flat `names` file
/// never carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemNamespace {
    /// A runtime value: a function or a const.
    Value,
    /// A type: a struct, enum, or type alias.
    Type,
    /// A trait.
    Trait,
    /// An ability.
    Ability,
    /// A module (reserved for whole-module records; not emitted per-item).
    Module,
}

impl ItemNamespace {
    /// The lowercase label used in human output and JSON.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Value => "value",
            Self::Type => "type",
            Self::Trait => "trait",
            Self::Ability => "ability",
            Self::Module => "module",
        }
    }
}

/// The precise kind of a structured item. A finer classification than
/// [`ItemNamespace`] (which it derives): `struct`/`enum`/`type` all share the
/// `type` namespace but read very differently in `store show`/`ls`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKindTag {
    /// A `fn` or `extern fn`.
    Function,
    /// A `const`.
    Const,
    /// A `struct`.
    Struct,
    /// An `enum`.
    Enum,
    /// A `type` alias.
    Alias,
    /// A `trait`.
    Trait,
    /// An `ability`.
    Ability,
}

impl ItemKindTag {
    /// The persisted discriminant. Stable across manifest versions; never
    /// renumber a variant.
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Function => 0,
            Self::Const => 1,
            Self::Struct => 2,
            Self::Enum => 3,
            Self::Alias => 4,
            Self::Trait => 5,
            Self::Ability => 6,
        }
    }

    /// Decode a persisted discriminant.
    #[must_use]
    pub const fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Function),
            1 => Some(Self::Const),
            2 => Some(Self::Struct),
            3 => Some(Self::Enum),
            4 => Some(Self::Alias),
            5 => Some(Self::Trait),
            6 => Some(Self::Ability),
            _ => None,
        }
    }

    /// The lowercase keyword-ish label for human output and `--kinds`
    /// filtering.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Function => "fn",
            Self::Const => "const",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Alias => "type",
            Self::Trait => "trait",
            Self::Ability => "ability",
        }
    }

    /// The namespace this kind belongs to.
    #[must_use]
    pub const fn namespace(self) -> ItemNamespace {
        match self {
            Self::Function | Self::Const => ItemNamespace::Value,
            Self::Struct | Self::Enum | Self::Alias => ItemNamespace::Type,
            Self::Trait => ItemNamespace::Trait,
            Self::Ability => ItemNamespace::Ability,
        }
    }

    /// Whether this kind is a runtime value (function or const) whose
    /// identity is a content hash rather than a nominal uuid.
    #[must_use]
    pub const fn is_value(self) -> bool {
        matches!(self, Self::Function | Self::Const)
    }
}

/// One top-level item, structured and spanned. The object hash (`None` here)
/// is filled by the manifest for value items; nominal kinds carry their uuid
/// in [`Self::uuid`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredItem {
    /// The item's ident path relative to its module (a single segment for a
    /// top-level item). Combined with the enclosing module's identity this
    /// reconstructs the item's `Fqn`.
    pub ident: Vec<Arc<str>>,
    /// The precise item kind.
    pub kind: ItemKindTag,
    /// The nominal uuid (`struct`/`enum`/`trait`/`ability`), rendered; empty
    /// for aliases, functions, consts, and structural structs.
    pub uuid: String,
    /// The definition's byte range `(start, end)` in the module source.
    pub span: (u32, u32),
    /// A one-line shape/signature summary for human inspection.
    pub summary: String,
}

impl StructuredItem {
    /// The item's own name — the last ident segment.
    #[must_use]
    pub fn name(&self) -> &str {
        self.ident.last().map_or("", |s| s.as_ref())
    }
}

/// Build the structured item index of a resolved module: every top-level
/// `fn`/`extern fn`/`const`/`struct`/`enum`/`type`/`trait`/`ability`, sorted
/// by name then kind for determinism. `use` and `impl` items are excluded
/// (they name nothing new).
#[must_use]
pub fn structured_items(module: &Module) -> Vec<StructuredItem> {
    let mut out = Vec::new();
    for item in &module.items {
        let Some(record) = item_record(item.span, &item.kind) else {
            continue;
        };
        out.push(record);
    }
    out.sort_by(|a, b| {
        a.ident
            .cmp(&b.ident)
            .then_with(|| a.kind.as_u8().cmp(&b.kind.as_u8()))
    });
    out
}

fn item_record(span: crate::ast::Span, kind: &ItemKind) -> Option<StructuredItem> {
    let span = (span.start, span.end);
    let record =
        |name: &Arc<str>, kind: ItemKindTag, uuid: String, summary: String| StructuredItem {
            ident: vec![Arc::clone(name)],
            kind,
            uuid,
            span,
            summary,
        };
    Some(match kind {
        ItemKind::Function(f) => record(
            &f.name,
            ItemKindTag::Function,
            String::new(),
            fn_summary(&f.params, f.ret_ty.as_ref()),
        ),
        ItemKind::ExternFn(e) => record(
            &e.name,
            ItemKindTag::Function,
            String::new(),
            format!("extern {}", fn_summary(&e.params, Some(&e.ret_ty))),
        ),
        ItemKind::Const(c) => record(
            &c.name,
            ItemKindTag::Const,
            String::new(),
            c.ty.clone()
                .or_else(|| crate::const_eval::literal_type(&c.value))
                .map_or_else(|| "_".to_string(), |t| render_type(&t)),
        ),
        ItemKind::Struct(s) => record(
            &s.name,
            ItemKindTag::Struct,
            s.unique_id.map(|u| u.to_string()).unwrap_or_default(),
            struct_summary(s),
        ),
        ItemKind::Enum(e) => record(
            &e.name,
            ItemKindTag::Enum,
            e.uuid.to_string(),
            enum_summary(e),
        ),
        ItemKind::TypeAlias(t) => record(
            &t.name,
            ItemKindTag::Alias,
            String::new(),
            format!("= {}", render_type(&t.ty)),
        ),
        ItemKind::Trait(t) => record(
            &t.name,
            ItemKindTag::Trait,
            t.uuid.to_string(),
            trait_summary(t),
        ),
        ItemKind::Ability(a) => record(
            &a.name,
            ItemKindTag::Ability,
            a.uuid.to_string(),
            ability_summary(a),
        ),
        ItemKind::Use(_) | ItemKind::Impl(_) => return None,
    })
}

fn fn_summary(params: &[Param], ret: Option<&crate::types::Type>) -> String {
    let params: Vec<String> = params
        .iter()
        .map(|p| render_type(p.declared_ty()))
        .collect();
    let ret = ret.map_or_else(|| "_".to_string(), render_type);
    format!("({}) -> {ret}", params.join(", "))
}

fn struct_summary(s: &crate::ast::StructDef) -> String {
    let fields = super::record_fields(&s.ty);
    let prefix = if s.is_extern { "extern " } else { "" };
    if fields.is_empty() {
        return format!("{prefix}unit");
    }
    let rendered: Vec<String> = fields
        .iter()
        .map(|(n, t)| format!("{n}: {}", render_type(t)))
        .collect();
    format!("{prefix}{{ {} }}", rendered.join(", "))
}

fn enum_summary(e: &crate::ast::EnumDef) -> String {
    let variants: Vec<String> = e
        .variants
        .iter()
        .map(|v| match &v.payload {
            Some(ty) => format!("{}({})", v.name, render_type(ty)),
            None => v.name.to_string(),
        })
        .collect();
    variants.join(" | ")
}

fn trait_summary(t: &crate::ast::TraitDef) -> String {
    let methods: Vec<String> = t
        .methods
        .iter()
        .map(|m| {
            let params: Vec<String> = m.params.iter().map(|(_, ty)| render_type(ty)).collect();
            format!(
                "{}({}) -> {}",
                m.name,
                params.join(", "),
                render_type(&m.ret_ty)
            )
        })
        .collect();
    format!("{{ {} }}", methods.join("; "))
}

fn ability_summary(a: &crate::ast::AbilityDef) -> String {
    let methods: Vec<String> = a
        .methods
        .iter()
        .map(|m| {
            let params: Vec<String> = m
                .params
                .iter()
                .map(|p| render_type(p.declared_ty()))
                .collect();
            format!(
                "{}({}) -> {}",
                m.name,
                params.join(", "),
                render_type(&m.ret_ty)
            )
        })
        .collect();
    format!("{{ {} }}", methods.join("; "))
}

//! Export extraction: what a module's items expose to importers.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{ItemKind, Module, Span, UsePrefix};
use crate::module_path::ModulePath;

/// Information about an exported symbol.
#[derive(Debug, Clone)]
pub struct ExportInfo {
    /// The name of the symbol.
    pub name: Arc<str>,
    /// The kind of symbol (function, const, type, enum, variant, ability, trait).
    pub kind: ExportKind,
    /// Whether the symbol is public (declared with `pub`). Enum variants
    /// inherit their enum's visibility.
    pub is_public: bool,
    /// If this is a re-export, the original module path.
    pub re_export_from: Option<ModulePath>,
    /// The span of the defining name in the defining module's source.
    /// Serves go-to-definition; enum variants use their variant span.
    pub name_span: Span,
    /// The item's doc comment, if any (enum variants inherit none).
    pub doc: Option<Arc<str>>,
    /// For an [`ExportKind::AbilityMethod`], the name of the ability that
    /// declares the method (in the defining module). `None` for every
    /// other kind. Carried so a method binding keeps naming *the* ability
    /// the user spelled — two same-module abilities may share a method
    /// name.
    pub owner: Option<Arc<str>>,
}

/// The kind of exported symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    Function,
    Const,
    Struct,
    TypeAlias,
    Enum,
    EnumVariant,
    Ability,
    /// A named ability set (`set IO = ...`). Occupies the ability namespace:
    /// a set appears only in ability positions and is interchangeable with an
    /// ability name there (it expands to its members).
    Set,
    /// One method of an ability, importable only through the explicit
    /// `use m::Ability::method;` path shape (never a module-level symbol —
    /// [`extract_exports`] does not emit these; they are synthesized by
    /// [`ModuleRegistry::lookup_ability_method`](super::ModuleRegistry::lookup_ability_method)).
    /// Importing one lets a perform site drop the ability prefix:
    /// `seed!(…)` for `Random::seed!(…)`.
    AbilityMethod,
    Trait,
}

/// A re-export (`pub use`), one flattened leaf.
#[derive(Debug, Clone)]
pub struct ReExport {
    /// The prefix of the import.
    pub prefix: UsePrefix,
    /// The path being re-exported.
    pub path: Vec<Arc<str>>,
    /// The exported name when renamed with `as`.
    pub alias: Option<Arc<str>>,
}

impl ReExport {
    /// The local name this re-export exposes: the alias if renamed, else
    /// the final path segment.
    #[must_use]
    pub fn exported_name(&self) -> Option<&str> {
        self.alias
            .as_deref()
            .or_else(|| self.path.last().map(AsRef::as_ref))
    }
}

/// A direct (non-method) export: every kind here is a module-level item,
/// so `owner` is always `None`.
fn direct(
    name: &Arc<str>,
    kind: ExportKind,
    is_public: bool,
    name_span: Span,
    doc: Option<Arc<str>>,
) -> ExportInfo {
    ExportInfo {
        name: Arc::clone(name),
        kind,
        is_public,
        re_export_from: None,
        name_span,
        doc,
        owner: None,
    }
}

/// Extract exports from a module.
pub(super) fn extract_exports(module: &Module) -> HashMap<Arc<str>, ExportInfo> {
    let mut exports = HashMap::new();

    for item in &module.items {
        let doc = item.doc.clone();
        let info = match &item.kind {
            ItemKind::Function(f) => Some(direct(
                &f.name,
                ExportKind::Function,
                f.is_public,
                f.name_span,
                doc,
            )),
            ItemKind::Const(c) => Some(direct(
                &c.name,
                ExportKind::Const,
                c.is_public,
                c.name_span,
                doc,
            )),
            ItemKind::Struct(s) => Some(direct(
                &s.name,
                ExportKind::Struct,
                s.is_public,
                s.name_span,
                doc,
            )),
            ItemKind::TypeAlias(t) => Some(direct(
                &t.name,
                ExportKind::TypeAlias,
                t.is_public,
                t.name_span,
                doc,
            )),
            ItemKind::Set(s) => Some(direct(
                &s.name,
                ExportKind::Set,
                s.is_public,
                s.name_span,
                doc,
            )),
            ItemKind::Enum(e) => {
                // Add the enum itself
                exports.insert(
                    e.name.clone(),
                    direct(&e.name, ExportKind::Enum, e.is_public, e.name_span, doc),
                );
                // Variants inherit the enum's visibility (and no doc).
                for variant in &e.variants {
                    exports.insert(
                        variant.name.clone(),
                        direct(
                            &variant.name,
                            ExportKind::EnumVariant,
                            e.is_public,
                            variant.span,
                            None,
                        ),
                    );
                }
                None // Already added
            }
            ItemKind::Ability(a) => Some(direct(
                &a.name,
                ExportKind::Ability,
                a.is_public,
                a.name_span,
                doc,
            )),
            ItemKind::Trait(t) => Some(direct(
                &t.name,
                ExportKind::Trait,
                t.is_public,
                t.name_span,
                doc,
            )),
            // Extern fns export exactly like functions: the missing body is
            // a compile-time binding concern, invisible to importers.
            ItemKind::ExternFn(e) => Some(direct(
                &e.name,
                ExportKind::Function,
                e.is_public,
                e.name_span,
                doc,
            )),
            ItemKind::Use(_) | ItemKind::Impl(_) => None, // Use statements and impls are not exports
        };

        if let Some(info) = info {
            exports.insert(info.name.clone(), info);
        }
    }

    exports
}

/// Extract re-exports from a module (`pub use` statements).
pub(super) fn extract_re_exports(module: &Module) -> Vec<ReExport> {
    let mut re_exports = Vec::new();

    for item in &module.items {
        if let ItemKind::Use(use_def) = &item.kind
            && use_def.is_public
        {
            re_exports.push(ReExport {
                prefix: use_def.prefix,
                path: use_def.path.iter().map(|(name, _)| name.clone()).collect(),
                alias: use_def.alias.as_ref().map(|(name, _)| name.clone()),
            });
        }
    }

    re_exports
}

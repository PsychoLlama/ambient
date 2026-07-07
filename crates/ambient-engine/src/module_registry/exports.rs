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

/// Extract exports from a module.
pub(super) fn extract_exports(module: &Module) -> HashMap<Arc<str>, ExportInfo> {
    let mut exports = HashMap::new();

    for item in &module.items {
        let info = match &item.kind {
            ItemKind::Function(f) => Some(ExportInfo {
                name: f.name.clone(),
                kind: ExportKind::Function,
                is_public: f.is_public,
                re_export_from: None,
                name_span: f.name_span,
                doc: item.doc.clone(),
            }),
            ItemKind::Const(c) => Some(ExportInfo {
                name: c.name.clone(),
                kind: ExportKind::Const,
                is_public: c.is_public,
                re_export_from: None,
                name_span: c.name_span,
                doc: item.doc.clone(),
            }),
            ItemKind::Struct(s) => Some(ExportInfo {
                name: s.name.clone(),
                kind: ExportKind::Struct,
                is_public: s.is_public,
                re_export_from: None,
                name_span: s.name_span,
                doc: item.doc.clone(),
            }),
            ItemKind::TypeAlias(t) => Some(ExportInfo {
                name: t.name.clone(),
                kind: ExportKind::TypeAlias,
                is_public: t.is_public,
                re_export_from: None,
                name_span: t.name_span,
                doc: item.doc.clone(),
            }),
            ItemKind::Enum(e) => {
                // Add the enum itself
                exports.insert(
                    e.name.clone(),
                    ExportInfo {
                        name: e.name.clone(),
                        kind: ExportKind::Enum,
                        is_public: e.is_public,
                        re_export_from: None,
                        name_span: e.name_span,
                        doc: item.doc.clone(),
                    },
                );

                // Variants inherit the enum's visibility.
                for variant in &e.variants {
                    exports.insert(
                        variant.name.clone(),
                        ExportInfo {
                            name: variant.name.clone(),
                            kind: ExportKind::EnumVariant,
                            is_public: e.is_public,
                            re_export_from: None,
                            name_span: variant.span,
                            doc: None,
                        },
                    );
                }
                None // Already added
            }
            ItemKind::Ability(a) => Some(ExportInfo {
                name: a.name.clone(),
                kind: ExportKind::Ability,
                is_public: a.is_public,
                re_export_from: None,
                name_span: a.name_span,
                doc: item.doc.clone(),
            }),
            ItemKind::Trait(t) => Some(ExportInfo {
                name: t.name.clone(),
                kind: ExportKind::Trait,
                is_public: t.is_public,
                re_export_from: None,
                name_span: t.name_span,
                doc: item.doc.clone(),
            }),
            // Extern fns export exactly like functions: the missing body is
            // a compile-time binding concern, invisible to importers.
            ItemKind::ExternFn(e) => Some(ExportInfo {
                name: e.name.clone(),
                kind: ExportKind::Function,
                is_public: e.is_public,
                re_export_from: None,
                name_span: e.name_span,
                doc: item.doc.clone(),
            }),
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

//! [`QualifiedName`]: the AST's reference to a named item.

use std::sync::Arc;

use super::Span;
use crate::fqn::{Fqn, ModuleId, NameKey};

/// A reference to a named item (function, type, ability).
#[derive(Debug, Clone)]
pub struct QualifiedName {
    /// Module path segments (empty for local names). A workspace-rooted
    /// reference (`::other_pkg::item`) spells its leading `::` as an empty
    /// head segment — the same convention `use ::` resolution keys on — so
    /// spelled equality, [`Self::joined`] rendering, and hashing all
    /// distinguish `::foo::x` from `foo::x` without a separate flag.
    pub path: Vec<Arc<str>>,
    /// Source spans for each path segment (for IDE features).
    /// Same length as `path`, or empty if spans are not available.
    pub path_spans: Vec<Span>,
    /// The final name.
    pub name: Arc<str>,
    /// Source span for the name (for IDE features).
    pub name_span: Option<Span>,
    /// Canonical target, filled in by the resolve pass — the item's
    /// fully-qualified location identity. Source spelling (`path`/`name`)
    /// is preserved for IDE features; consumers that need the semantic
    /// target go through [`QualifiedName::resolution_key`]. `None` means
    /// the reference is a local binding, or the module was checked without
    /// a registry.
    pub resolved: Option<Fqn>,
}

impl PartialEq for QualifiedName {
    fn eq(&self, other: &Self) -> bool {
        // Ignore spans and resolution for equality - only compare the
        // spelled semantic content.
        self.path == other.path && self.name == other.name
    }
}

impl Eq for QualifiedName {}

impl QualifiedName {
    /// The full qualified form of this name (`core::collections::list::map`), or just
    /// the name when the path is empty.
    #[must_use]
    pub fn joined(&self) -> Arc<str> {
        if self.path.is_empty() {
            return Arc::clone(&self.name);
        }
        let mut s = String::new();
        for segment in &self.path {
            s.push_str(segment);
            s.push_str("::");
        }
        s.push_str(&self.name);
        s.into()
    }

    /// The key this reference is bound under in checker environments and
    /// linker tables: its fully-qualified [`Fqn`] identity when resolved,
    /// else the spelled qualified string as a bare key (a local binding or
    /// a registry-less reference).
    #[must_use]
    pub fn resolution_key(&self) -> NameKey {
        self.resolved.as_ref().map_or_else(
            || NameKey::Bare(self.joined()),
            |fqn| NameKey::Item(fqn.clone()),
        )
    }

    /// The resolved item's defining module, if resolved. Ability
    /// resolution keys off this.
    #[must_use]
    pub fn resolved_module_id(&self) -> Option<&ModuleId> {
        self.resolved.as_ref().map(|fqn| &fqn.module)
    }

    /// The [`Fqn`] an intrinsic reference resolves to, if any: the
    /// canonicalized `Fqn` when the resolve pass ran, else one
    /// reconstructed from the spelled path (intrinsics are always
    /// `core`-rooted, so the scope is `Builtin` regardless of the
    /// workspace). `None` for a bare reference, which is never an
    /// intrinsic.
    #[must_use]
    pub fn intrinsic_fqn(&self) -> Option<Fqn> {
        if let Some(fqn) = &self.resolved {
            return Some(fqn.clone());
        }
        if self.path.is_empty() {
            return None;
        }
        let segments: Vec<&str> = self.path.iter().map(AsRef::as_ref).collect();
        Some(Fqn::new(
            ModuleId::from_dotted_segments(&segments, &Arc::from("")),
            vec![Arc::clone(&self.name)],
        ))
    }

    /// The item name at the resolved target (aliases unfolded, the final
    /// ident segment), else the spelled name.
    #[must_use]
    pub fn resolved_name(&self) -> &str {
        self.resolved
            .as_ref()
            .map_or_else(|| self.name.as_ref(), Fqn::name)
    }

    /// Whether two references name the same target: by resolved [`Fqn`]
    /// identity when both were canonicalized, else by spelled path+name (the
    /// registry-less fallback, matching this type's own `PartialEq`). Trait
    /// bounds dedup and conformance-check through this, so a qualified
    /// re-spelling collapses onto a bare one and two same-named traits from
    /// different modules stay distinct.
    #[must_use]
    pub fn same_target(&self, other: &Self) -> bool {
        match (&self.resolved, &other.resolved) {
            (Some(a), Some(b)) => a == b,
            _ => self == other,
        }
    }

    /// Create a simple unqualified name.
    #[must_use]
    pub fn simple(name: impl Into<Arc<str>>) -> Self {
        Self {
            path: Vec::new(),
            path_spans: Vec::new(),
            name: name.into(),
            name_span: None,
            resolved: None,
        }
    }

    /// Create a qualified name with path.
    #[must_use]
    pub fn qualified(path: Vec<impl Into<Arc<str>>>, name: impl Into<Arc<str>>) -> Self {
        Self {
            path: path.into_iter().map(Into::into).collect(),
            path_spans: Vec::new(),
            name: name.into(),
            name_span: None,
            resolved: None,
        }
    }

    /// Create a qualified name with full span information.
    #[must_use]
    pub fn with_spans(
        path: Vec<Arc<str>>,
        path_spans: Vec<Span>,
        name: Arc<str>,
        name_span: Span,
    ) -> Self {
        Self {
            path,
            path_spans,
            name,
            name_span: Some(name_span),
            resolved: None,
        }
    }
}

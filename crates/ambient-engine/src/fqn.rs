//! Fully-qualified names (FQNs): the unambiguous, first-class identity of
//! every item a build contains.
//!
//! An item's identity used to be a *joined string* (`"utils::format"`,
//! `"core::primitives::number::sqrt"`), which could not say whether a
//! middle segment was a submodule or a type, carried no package identity,
//! and forced every internal table to compare strings. This module makes
//! the identity a real data structure that every built-in and every user
//! item has, keyed off `Eq`/`Hash` — never a joined string.
//!
//! # Two identity axes
//!
//! FQNs are the **location** axis only — the identity of anything a `use`
//! path names and the resolve pass canonicalizes (top-level items,
//! type-associated members, enum variants). The **content** axis —
//! UUID-based method dispatch symbols (`<type-uuid>::method`) — is *not*
//! folded in; where a perfect content identity exists, it reigns. See
//! [`NameKey`], which unifies both as a lookup key.
//!
//! # Structure
//!
//! - [`Scope`]: whose namespace the item lives in — `Builtin` (`core`) or
//!   a named [`Workspace`](Scope::Workspace) package. (`Library(hash)` is
//!   reserved for the future.)
//! - [`ModuleId`]: a scope plus a scope-relative module path.
//! - [`Fqn`]: a [`ModuleId`] plus an ident path of one or more segments.
//!
//! The `Display` impls exist only for humans (diagnostics) and the single
//! on-disk consumer (`.ambient/store/names`). No internal code compares
//! `Display` output.

use std::fmt;
use std::sync::Arc;

use crate::module_path::ModulePath;

/// Whose namespace an item lives in.
///
/// `Builtin` is the reserved `core` standard library; `Workspace` is a
/// named user package. `Library(hash)` — a content-addressed dependency —
/// is intentionally left for the future; do not add it as dead code.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    /// The reserved `core` standard library.
    Builtin,
    /// A named user package (its `ambient.toml` `name`).
    Workspace(Arc<str>),
    // Future: Library(Hash) — a content-addressed dependency.
}

/// A module's identity: a [`Scope`] plus the module path *relative to that
/// scope*.
///
/// For a `core` module the leading `core` segment is stripped and folded
/// into [`Scope::Builtin`]; for a user module the full package-relative
/// path is kept under [`Scope::Workspace`]. The registry and dependency
/// graph key on `ModuleId`; items key on [`Fqn`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId {
    /// Whose namespace the module lives in.
    pub scope: Scope,
    /// Scope-relative module path segments (`["primitives"]` for
    /// `core::primitives`, `["utils"]` for a user `utils` module).
    pub path: Vec<Arc<str>>,
}

impl ModuleId {
    /// Build a [`ModuleId`] from a [`ModulePath`] and the current
    /// workspace package name.
    ///
    /// A leading `core` segment folds into [`Scope::Builtin`] (and is
    /// stripped from `path`); every other path is scoped to the workspace.
    #[must_use]
    pub fn from_module_path(path: &ModulePath, workspace: &Arc<str>) -> Self {
        let (is_builtin, segments) = path.scope_relative();
        if is_builtin {
            Self {
                scope: Scope::Builtin,
                path: segments.to_vec(),
            }
        } else {
            Self {
                scope: Scope::Workspace(Arc::clone(workspace)),
                path: segments.to_vec(),
            }
        }
    }

    /// A builtin (`core`) module from its scope-relative segments
    /// (`["primitives"]` for `core::primitives`). The engine uses this to
    /// name intrinsic and platform modules directly.
    #[must_use]
    pub fn builtin(segments: &[&str]) -> Self {
        Self {
            scope: Scope::Builtin,
            path: segments.iter().map(|s| Arc::from(*s)).collect(),
        }
    }

    /// The `core::system` declaration module — the namespace embedder
    /// platform abilities (`Stdio`, `Tcp`, ...) are registered under.
    #[must_use]
    pub fn core_system() -> Self {
        Self::builtin(&["system"])
    }

    /// The dotted [`ModulePath`]-style key this module has as a plain module
    /// path (`a::top`, `core::primitives`) — the inverse of
    /// [`Self::from_module_path`]'s scoping, dropping the `workspace::<pkg>`
    /// framing. Used to reconcile `Fqn`-based dependency edges with the
    /// build's `ModulePath`-keyed compilation-order graph.
    #[must_use]
    pub fn module_path_string(&self) -> String {
        let mut segments: Vec<&str> = Vec::new();
        if matches!(self.scope, Scope::Builtin) {
            segments.push("core");
        }
        segments.extend(self.path.iter().map(AsRef::as_ref));
        segments.join("::")
    }

    /// Build a [`ModuleId`] from dotted namespace segments (`["core",
    /// "system"]`, `["utils"]`) under `workspace`, applying the same
    /// `core`-stripping rule as [`Self::from_module_path`]. Used to resolve
    /// `::`-joined ability annotations that never became [`ModulePath`]s.
    #[must_use]
    pub fn from_dotted_segments(segments: &[&str], workspace: &Arc<str>) -> Self {
        if matches!(segments.first(), Some(&"core")) {
            Self {
                scope: Scope::Builtin,
                path: segments[1..].iter().map(|s| Arc::from(*s)).collect(),
            }
        } else {
            Self {
                scope: Scope::Workspace(Arc::clone(workspace)),
                path: segments.iter().map(|s| Arc::from(*s)).collect(),
            }
        }
    }

    /// The [`ModulePath`] this module is keyed under in the registry — the
    /// structured inverse of [`Self::from_module_path`], re-attaching the
    /// leading `core` segment for a builtin and dropping the
    /// `workspace::<pkg>` framing for a user module. `None` only for the
    /// degenerate empty-path module (which the registry never holds).
    #[must_use]
    pub fn to_module_path(&self) -> Option<ModulePath> {
        let mut segments: Vec<Arc<str>> = Vec::new();
        if matches!(self.scope, Scope::Builtin) {
            segments.push(Arc::from("core"));
        }
        segments.extend(self.path.iter().cloned());
        ModulePath::from_segments(segments)
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.scope {
            Scope::Builtin => write!(f, "core")?,
            Scope::Workspace(pkg) => write!(f, "workspace::{pkg}")?,
        }
        for segment in &self.path {
            write!(f, "::{segment}")?;
        }
        Ok(())
    }
}

/// The fully-qualified identity of an item: a [`ModuleId`] plus an ident
/// path of one or more segments.
///
/// The ident path is a plain sequence — no namespace tag. Values, types,
/// and abilities already live in separate keyed tables (`Namespace`), so a
/// same-named value and type in one module legitimately share an `Fqn`;
/// the lookup picks the table by syntactic position.
///
/// - A top-level item is one ident segment: `utils::format` →
///   `ident = ["format"]`.
/// - A type-associated member is two: `core::primitives::number::sqrt` →
///   module `core::primitives`, `ident = ["number", "sqrt"]`.
/// - An enum variant is two: `ident = [Enum, Variant]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fqn {
    /// The defining module.
    pub module: ModuleId,
    /// The ident path (at least one segment).
    pub ident: Vec<Arc<str>>,
}

impl Fqn {
    /// Build an [`Fqn`] from a module and its ident-path segments.
    #[must_use]
    pub fn new(module: ModuleId, ident: Vec<Arc<str>>) -> Self {
        Self { module, ident }
    }

    /// The final ident segment — the item's own name (`sqrt` in
    /// `Number::sqrt`, `format` in `utils::format`).
    #[must_use]
    pub fn name(&self) -> &str {
        // An `Fqn` always has at least one ident segment.
        self.ident.last().map_or("", |s| s.as_ref())
    }
}

impl fmt::Display for Fqn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.module)?;
        for segment in &self.ident {
            write!(f, "::{segment}")?;
        }
        Ok(())
    }
}

/// The lookup key for the checker env, linker, and every internal item
/// table: either a fully-qualified location [`Fqn`], or a bare string.
///
/// The `Item` variant is the location axis: genuine cross-module item
/// identity, compared by `Fqn`'s `Eq`/`Hash` (never a joined string). The
/// `Bare` variant covers everything that legitimately keys on a string —
/// content-addressed dispatch symbols (`<uuid>::method`), local bindings,
/// and the registry-less REPL/test fallback. The two never collide: a
/// resolved item is always `Item`, and content symbols and locals are
/// disjoint from any workspace-qualified spelling.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NameKey {
    /// A bare string key: a content-addressed symbol, a local binding, or
    /// an unresolved/registry-less reference.
    Bare(Arc<str>),
    /// A resolved location identity.
    Item(Fqn),
}

impl fmt::Display for NameKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bare(s) => write!(f, "{s}"),
            Self::Item(fqn) => write!(f, "{fqn}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp(segments: &[&str]) -> ModulePath {
        ModulePath::from_str_segments(segments).unwrap()
    }

    #[test]
    fn builtin_strips_core() {
        let ws = Arc::from("my_pkg");
        let id = ModuleId::from_module_path(&mp(&["core", "primitives"]), &ws);
        assert_eq!(id.scope, Scope::Builtin);
        assert_eq!(id.path, vec![Arc::from("primitives")]);
        assert_eq!(id.to_string(), "core::primitives");
    }

    #[test]
    fn workspace_keeps_full_path() {
        let ws: Arc<str> = Arc::from("my_pkg");
        let id = ModuleId::from_module_path(&mp(&["foo", "bar"]), &ws);
        assert_eq!(id.scope, Scope::Workspace(Arc::clone(&ws)));
        assert_eq!(id.path, vec![Arc::from("foo"), Arc::from("bar")]);
        assert_eq!(id.to_string(), "workspace::my_pkg::foo::bar");
    }

    #[test]
    fn display_builtin_type_associated() {
        let fqn = Fqn::new(
            ModuleId::builtin(&["primitives"]),
            vec![Arc::from("number"), Arc::from("sqrt")],
        );
        assert_eq!(fqn.to_string(), "core::primitives::number::sqrt");
        assert_eq!(fqn.name(), "sqrt");
    }

    #[test]
    fn display_workspace_item() {
        let ws = Arc::from("my_example");
        let fqn = Fqn::new(
            ModuleId::from_module_path(&mp(&["foo"]), &ws),
            vec![Arc::from("bar"), Arc::from("baz")],
        );
        assert_eq!(fqn.to_string(), "workspace::my_example::foo::bar::baz");
    }

    /// `a::b::c` is ambiguous as a string but distinct as an `Fqn`: `b` a
    /// submodule (value `c` in module `a::b`) vs `b` a type (member `c` of
    /// type `b` in module `a`) yield different structures.
    #[test]
    fn submodule_vs_type_associated_are_distinct() {
        let ws = Arc::from("pkg");
        let submodule = Fqn::new(
            ModuleId::from_module_path(&mp(&["a", "b"]), &ws),
            vec![Arc::from("c")],
        );
        let type_associated = Fqn::new(
            ModuleId::from_module_path(&mp(&["a"]), &ws),
            vec![Arc::from("b"), Arc::from("c")],
        );
        assert_ne!(submodule, type_associated);
        // Yet they render identically for humans.
        assert_eq!(submodule.to_string(), type_associated.to_string());
    }

    #[test]
    fn enum_variant_is_two_segment_ident() {
        let ws = Arc::from("pkg");
        let variant = Fqn::new(
            ModuleId::from_module_path(&mp(&["shapes"]), &ws),
            vec![Arc::from("Color"), Arc::from("Red")],
        );
        assert_eq!(variant.ident.len(), 2);
        assert_eq!(variant.name(), "Red");
        assert_eq!(variant.to_string(), "workspace::pkg::shapes::Color::Red");
    }

    #[test]
    fn name_key_item_compares_by_fqn() {
        let ws = Arc::from("pkg");
        let a = NameKey::Item(Fqn::new(
            ModuleId::from_module_path(&mp(&["m"]), &ws),
            vec![Arc::from("f")],
        ));
        let b = NameKey::Item(Fqn::new(
            ModuleId::from_module_path(&mp(&["m"]), &ws),
            vec![Arc::from("f")],
        ));
        assert_eq!(a, b);
        assert_ne!(a, NameKey::Bare(Arc::from("workspace::pkg::m::f")));
    }
}

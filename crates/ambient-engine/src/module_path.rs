//! Module path representation and resolution.
//!
//! A module path identifies a module within a package. It mirrors the
//! filesystem structure relative to the package's source directory.
//!
//! # Examples
//!
//! ```text
//! src/main.ab           -> ModulePath(["main"])
//! src/utils.ab          -> ModulePath(["utils"])
//! src/utils/format.ab   -> ModulePath(["utils", "format"])
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;

/// A path to a module within a package.
///
/// Module paths are sequences of identifiers that correspond to the
/// filesystem path relative to the source directory, without the `.ab`
/// extension.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModulePath {
    /// Path segments (e.g., `["utils", "format"]`).
    segments: Vec<Arc<str>>,
}

/// Errors that can occur during module path resolution.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ResolutionError {
    /// Import would escape above the package source root.
    #[error("import escapes package root (too many `super` segments)")]
    EscapedPackageRoot,

    /// The module path is empty.
    #[error("empty module path")]
    EmptyPath,
}

/// The prefix of an import path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportPrefix {
    /// Local package: `pkg::module`
    Pkg,
    /// Standard library: `core::module`
    Core,
    /// Same directory: `self::sibling`
    Self_,
    /// Parent directory: `super::parent`, `super::super::grandparent`
    /// The usize is how many levels up (1 for `super`, 2 for `super::super`, etc.)
    Super(usize),
}

impl ModulePath {
    /// Create the root module path (main.ab).
    #[must_use]
    pub fn root() -> Self {
        Self {
            segments: vec![Arc::from("main")],
        }
    }

    /// Create a module path from segments.
    ///
    /// Returns `None` if the segments are empty.
    #[must_use]
    pub fn from_segments(segments: Vec<Arc<str>>) -> Option<Self> {
        if segments.is_empty() {
            None
        } else {
            Some(Self { segments })
        }
    }

    /// Create a module path from string segments.
    ///
    /// Returns `None` if the segments are empty.
    #[must_use]
    pub fn from_str_segments(segments: &[&str]) -> Option<Self> {
        if segments.is_empty() {
            None
        } else {
            Some(Self {
                segments: segments.iter().map(|s| Arc::from(*s)).collect(),
            })
        }
    }

    /// Get the path segments.
    #[must_use]
    pub fn segments(&self) -> &[Arc<str>] {
        &self.segments
    }

    /// Whether this path lives under the reserved root (`core`). User
    /// modules may not take this name: `core` is a keyword, so such a
    /// module could never be referenced — but worse, its canonical names
    /// would collide with the reserved namespace.
    #[must_use]
    pub fn collides_with_reserved_root(&self) -> bool {
        matches!(self.segments.first().map(AsRef::as_ref), Some("core"))
    }

    /// Split this path into `(is_builtin, scope-relative segments)`.
    ///
    /// A path rooted at the reserved `core` namespace is builtin; its
    /// leading `core` segment is stripped so the remainder is relative to
    /// [`crate::fqn::Scope::Builtin`]. Every other path is workspace-scoped
    /// and returned whole. This is the single split
    /// [`crate::fqn::ModuleId::from_module_path`] keys off.
    #[must_use]
    pub fn scope_relative(&self) -> (bool, &[Arc<str>]) {
        if matches!(self.segments.first().map(AsRef::as_ref), Some("core")) {
            (true, &self.segments[1..])
        } else {
            (false, &self.segments)
        }
    }

    /// Get the module name (last segment).
    #[must_use]
    pub fn name(&self) -> &str {
        // Safe because we never allow empty paths
        &self.segments[self.segments.len() - 1]
    }

    /// Get the parent module path.
    ///
    /// Returns `None` if this is a root-level module (single segment).
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        if self.segments.len() <= 1 {
            None
        } else {
            Some(Self {
                segments: self.segments[..self.segments.len() - 1].to_vec(),
            })
        }
    }

    /// Get the directory containing this module.
    ///
    /// For `["utils", "format"]`, returns `["utils"]`.
    /// For `["main"]`, returns `None` (root directory).
    #[must_use]
    pub fn containing_dir(&self) -> Option<Self> {
        self.parent()
    }

    /// The directory the module's file lives in — the anchor `self::`
    /// resolves against.
    ///
    /// A *directory module* is backed by a `main.ab` (e.g.
    /// `collections/main.ab` → `collections`); its file sits *inside* its
    /// own path, so `self` is the module's own path. Every other module is
    /// backed by a plain `<name>.ab` file whose directory is the parent.
    /// Returns `None` for the package root's own directory.
    #[must_use]
    pub fn file_dir(&self, is_dir_module: bool) -> Option<Self> {
        if is_dir_module {
            Some(self.clone())
        } else {
            self.parent()
        }
    }

    /// Append a segment to create a child module path.
    #[must_use]
    pub fn child(&self, name: impl Into<Arc<str>>) -> Self {
        let mut segments = self.segments.clone();
        segments.push(name.into());
        Self { segments }
    }

    /// Resolve a relative path from this module.
    ///
    /// `is_dir_module` says whether this module is backed by a `main.ab`
    /// (a directory module): it changes where `self`/`super` anchor. See
    /// [`ModulePath::file_dir`].
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The resolution would escape above the package root
    /// - The path is empty
    /// - The prefix is `Core` (handled separately)
    pub fn resolve_relative(
        &self,
        prefix: &ImportPrefix,
        path: &[Arc<str>],
        is_dir_module: bool,
    ) -> Result<ModulePath, ResolutionError> {
        if path.is_empty() {
            return Err(ResolutionError::EmptyPath);
        }

        match prefix {
            ImportPrefix::Pkg => {
                // Absolute path from package root
                Ok(ModulePath {
                    segments: path.to_vec(),
                })
            }
            ImportPrefix::Core => {
                // Core modules live under the reserved `core` root. `core`
                // is a lexer keyword, so user modules can never collide.
                let mut segments: Vec<Arc<str>> = vec![Arc::from("core")];
                segments.extend(path.iter().cloned());
                Ok(ModulePath { segments })
            }
            ImportPrefix::Self_ => {
                // The module's own directory (its path for a directory
                // module, else the parent).
                let dir = self.file_dir(is_dir_module);
                let mut segments = dir.map_or_else(Vec::new, |d| d.segments);
                segments.extend(path.iter().cloned());
                Ok(ModulePath { segments })
            }
            ImportPrefix::Super(count) => {
                // Start from the module's own directory, then step up
                // `count` further levels.
                let mut dir = self.file_dir(is_dir_module);
                for _ in 0..*count {
                    match dir {
                        Some(d) => dir = d.parent(),
                        None => return Err(ResolutionError::EscapedPackageRoot),
                    }
                }

                let mut segments = dir.map_or_else(Vec::new, |d| d.segments);
                segments.extend(path.iter().cloned());

                if segments.is_empty() {
                    return Err(ResolutionError::EmptyPath);
                }

                Ok(ModulePath { segments })
            }
        }
    }

    /// Convert to filesystem path relative to src directory.
    ///
    /// For `["utils", "format"]`, returns `utils/format.ab`.
    #[must_use]
    pub fn to_file_path(&self) -> PathBuf {
        let mut path = PathBuf::new();
        for segment in &self.segments {
            path.push(segment.as_ref());
        }
        path.set_extension("ab");
        path
    }

    /// The inverse of [`ModulePath::to_file_path`]: a module path from a
    /// filesystem path relative to the src directory.
    ///
    /// `utils/format.ab` → `["utils", "format"]`; `main.ab` → the root
    /// module; a directory's `main.ab` collapses to the directory
    /// (`collections/main.ab` → `["collections"]`). This is the single
    /// definition of the file↔module mapping — every tool that walks
    /// source trees must resolve paths through it, so the mapping can
    /// never fork.
    #[must_use]
    pub fn from_relative_file_path(relative: &std::path::Path) -> Option<Self> {
        Self::from_relative_file_path_with_kind(relative).map(|(path, _)| path)
    }

    /// [`ModulePath::from_relative_file_path`] plus whether the file is a
    /// directory module's `main.ab`.
    ///
    /// A trailing `main` segment collapses so `<dir>/main.ab` is the module
    /// `<dir>` (a directory module — the returned bool is `true`), at every
    /// nesting level. The package-root `main.ab` is the root module and is
    /// *not* a directory module (its siblings live at the root, so `self`
    /// anchors there, not under `main`).
    #[must_use]
    pub fn from_relative_file_path_with_kind(relative: &std::path::Path) -> Option<(Self, bool)> {
        let mut segments: Vec<Arc<str>> = Vec::new();
        for component in relative.components() {
            let std::path::Component::Normal(s) = component else {
                return None;
            };
            let name = s.to_str()?;
            let name = name.strip_suffix(".ab").unwrap_or(name);
            segments.push(Arc::from(name));
        }
        // A trailing `main` names its containing directory (a directory
        // module). At the package root this leaves no segments — the root
        // `main.ab`, which is an ordinary (non-directory) module.
        let is_dir_module = if segments.last().map(AsRef::as_ref) == Some("main") {
            segments.pop();
            !segments.is_empty()
        } else {
            false
        };
        if segments.is_empty() {
            Some((Self::root(), false))
        } else {
            Some((Self::from_segments(segments)?, is_dir_module))
        }
    }
}

impl std::fmt::Display for ModulePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, segment) in self.segments.iter().enumerate() {
            if i > 0 {
                write!(f, "::")?;
            }
            write!(f, "{segment}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_root() {
        let root = ModulePath::root();
        assert_eq!(root.segments(), &[Arc::from("main")]);
        assert_eq!(root.name(), "main");
        assert_eq!(root.to_string(), "main");
    }

    #[test]
    fn test_from_segments() {
        let path = ModulePath::from_str_segments(&["utils", "format"]).unwrap();
        assert_eq!(path.segments().len(), 2);
        assert_eq!(path.name(), "format");
        assert_eq!(path.to_string(), "utils::format");
    }

    #[test]
    fn test_from_empty_segments() {
        assert!(ModulePath::from_str_segments(&[]).is_none());
    }

    #[test]
    fn test_parent() {
        let path = ModulePath::from_str_segments(&["a", "b", "c"]).unwrap();
        let parent = path.parent().unwrap();
        assert_eq!(parent.to_string(), "a::b");

        let grandparent = parent.parent().unwrap();
        assert_eq!(grandparent.to_string(), "a");

        assert!(grandparent.parent().is_none());
    }

    #[test]
    fn test_child() {
        let path = ModulePath::from_str_segments(&["utils"]).unwrap();
        let child = path.child("format");
        assert_eq!(child.to_string(), "utils::format");
    }

    #[test]
    fn test_to_file_path() {
        let path = ModulePath::from_str_segments(&["utils", "format"]).unwrap();
        assert_eq!(path.to_file_path(), PathBuf::from("utils/format.ab"));
    }

    #[test]
    fn test_resolve_pkg() {
        let current = ModulePath::from_str_segments(&["foo", "bar"]).unwrap();
        let path = vec![Arc::from("utils"), Arc::from("helper")];

        let resolved = current
            .resolve_relative(&ImportPrefix::Pkg, &path, false)
            .unwrap();
        assert_eq!(resolved.to_string(), "utils::helper");
    }

    #[test]
    fn test_resolve_self() {
        let current = ModulePath::from_str_segments(&["foo", "bar"]).unwrap();
        let path = vec![Arc::from("sibling")];

        let resolved = current
            .resolve_relative(&ImportPrefix::Self_, &path, false)
            .unwrap();
        assert_eq!(resolved.to_string(), "foo::sibling");
    }

    #[test]
    fn test_resolve_self_from_root() {
        let current = ModulePath::from_str_segments(&["main"]).unwrap();
        let path = vec![Arc::from("sibling")];

        let resolved = current
            .resolve_relative(&ImportPrefix::Self_, &path, false)
            .unwrap();
        assert_eq!(resolved.to_string(), "sibling");
    }

    /// A directory module (`collections/main.ab` → `collections`) anchors
    /// `self` at its *own* path, not its parent.
    #[test]
    fn test_resolve_self_from_dir_module() {
        let current = ModulePath::from_str_segments(&["core", "collections"]).unwrap();
        let path = vec![Arc::from("List")];

        let resolved = current
            .resolve_relative(&ImportPrefix::Self_, &path, true)
            .unwrap();
        assert_eq!(resolved.to_string(), "core::collections::List");
    }

    /// From a directory module, `super` steps up from the module's own
    /// path (`core::collections` → `core`).
    #[test]
    fn test_resolve_super_from_dir_module() {
        let current = ModulePath::from_str_segments(&["core", "collections"]).unwrap();
        let path = vec![Arc::from("time")];

        let resolved = current
            .resolve_relative(&ImportPrefix::Super(1), &path, true)
            .unwrap();
        assert_eq!(resolved.to_string(), "core::time");
    }

    #[test]
    fn test_resolve_super() {
        let current = ModulePath::from_str_segments(&["a", "b", "c"]).unwrap();
        let path = vec![Arc::from("other")];

        let resolved = current
            .resolve_relative(&ImportPrefix::Super(1), &path, false)
            .unwrap();
        assert_eq!(resolved.to_string(), "a::other");
    }

    #[test]
    fn test_resolve_super_super() {
        let current = ModulePath::from_str_segments(&["a", "b", "c"]).unwrap();
        let path = vec![Arc::from("other")];

        let resolved = current
            .resolve_relative(&ImportPrefix::Super(2), &path, false)
            .unwrap();
        assert_eq!(resolved.to_string(), "other");
    }

    #[test]
    fn test_resolve_super_escapes_root() {
        let current = ModulePath::from_str_segments(&["a", "b"]).unwrap();
        let path = vec![Arc::from("other")];

        let err = current
            .resolve_relative(&ImportPrefix::Super(2), &path, false)
            .unwrap_err();
        assert!(matches!(err, ResolutionError::EscapedPackageRoot));
    }

    #[test]
    fn test_resolve_core() {
        let current = ModulePath::root();
        let path = vec![Arc::from("List")];

        let resolved = current
            .resolve_relative(&ImportPrefix::Core, &path, false)
            .expect("core paths resolve under the reserved `core` root");
        assert_eq!(resolved.to_string(), "core::List");
    }

    #[test]
    fn test_resolve_empty_path() {
        let current = ModulePath::root();

        let err = current
            .resolve_relative(&ImportPrefix::Pkg, &[], false)
            .unwrap_err();
        assert!(matches!(err, ResolutionError::EmptyPath));
    }

    /// A nested `main.ab` collapses to its directory and is flagged as a
    /// directory module; the package-root `main.ab` is the root module and
    /// is not.
    #[test]
    fn test_from_relative_file_path_dir_module() {
        use std::path::Path;

        let (path, is_dir) =
            ModulePath::from_relative_file_path_with_kind(Path::new("collections/main.ab"))
                .unwrap();
        assert_eq!(path.to_string(), "collections");
        assert!(is_dir);

        let (path, is_dir) =
            ModulePath::from_relative_file_path_with_kind(Path::new("collections/List.ab"))
                .unwrap();
        assert_eq!(path.to_string(), "collections::List");
        assert!(!is_dir);

        let (path, is_dir) =
            ModulePath::from_relative_file_path_with_kind(Path::new("main.ab")).unwrap();
        assert_eq!(path.to_string(), "main");
        assert!(!is_dir);
    }
}

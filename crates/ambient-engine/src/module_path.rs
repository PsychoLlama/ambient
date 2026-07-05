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
    /// Embedded platform ability module: `platform::Ability`
    Platform,
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

    /// Whether this path lives under a reserved root (`core`,
    /// `platform`). User modules may not take these names: `core` is a
    /// keyword and `platform` a contextual keyword, so such a module
    /// could never be referenced — but worse, its canonical names would
    /// collide with the reserved namespace.
    #[must_use]
    pub fn collides_with_reserved_root(&self) -> bool {
        matches!(
            self.segments.first().map(AsRef::as_ref),
            Some("core" | "platform")
        )
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

    /// Append a segment to create a child module path.
    #[must_use]
    pub fn child(&self, name: impl Into<Arc<str>>) -> Self {
        let mut segments = self.segments.clone();
        segments.push(name.into());
        Self { segments }
    }

    /// Resolve a relative path from this module.
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
            ImportPrefix::Platform => {
                // Platform abilities live under the reserved `platform`
                // root, registered as a declaration module by embedders.
                // `platform` is a contextual keyword in use-prefix position,
                // so a user module named `platform` never collides here.
                let mut segments: Vec<Arc<str>> = vec![Arc::from("platform")];
                segments.extend(path.iter().cloned());
                Ok(ModulePath { segments })
            }
            ImportPrefix::Self_ => {
                // Same directory as current module
                let dir = self.containing_dir();
                let mut segments = dir.map_or_else(Vec::new, |d| d.segments);
                segments.extend(path.iter().cloned());
                Ok(ModulePath { segments })
            }
            ImportPrefix::Super(count) => {
                // Go up `count` directories
                let mut segments = self.segments.clone();

                // Pop this module's name first
                segments.pop();

                // Then pop `count` more directories
                for _ in 0..*count {
                    if segments.is_empty() {
                        return Err(ResolutionError::EscapedPackageRoot);
                    }
                    segments.pop();
                }

                // Append the path
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
    /// `utils/format.ab` → `["utils", "format"]`; `main.ab` (or a bare
    /// `main` segment at the top level) → the root module. This is the
    /// single definition of the file↔module mapping — every tool that
    /// walks source trees must resolve paths through it, so the mapping
    /// can never fork.
    #[must_use]
    pub fn from_relative_file_path(relative: &std::path::Path) -> Option<Self> {
        let mut segments: Vec<Arc<str>> = Vec::new();
        for component in relative.components() {
            let std::path::Component::Normal(s) = component else {
                return None;
            };
            let name = s.to_str()?;
            let name = name.strip_suffix(".ab").unwrap_or(name);
            // `main.ab` at the package root is the root module.
            if name == "main" && segments.is_empty() {
                continue;
            }
            segments.push(Arc::from(name));
        }
        if segments.is_empty() {
            Some(Self::root())
        } else {
            Self::from_segments(segments)
        }
    }
}

impl std::fmt::Display for ModulePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, segment) in self.segments.iter().enumerate() {
            if i > 0 {
                write!(f, ".")?;
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
        assert_eq!(path.to_string(), "utils.format");
    }

    #[test]
    fn test_from_empty_segments() {
        assert!(ModulePath::from_str_segments(&[]).is_none());
    }

    #[test]
    fn test_parent() {
        let path = ModulePath::from_str_segments(&["a", "b", "c"]).unwrap();
        let parent = path.parent().unwrap();
        assert_eq!(parent.to_string(), "a.b");

        let grandparent = parent.parent().unwrap();
        assert_eq!(grandparent.to_string(), "a");

        assert!(grandparent.parent().is_none());
    }

    #[test]
    fn test_child() {
        let path = ModulePath::from_str_segments(&["utils"]).unwrap();
        let child = path.child("format");
        assert_eq!(child.to_string(), "utils.format");
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

        let resolved = current.resolve_relative(&ImportPrefix::Pkg, &path).unwrap();
        assert_eq!(resolved.to_string(), "utils.helper");
    }

    #[test]
    fn test_resolve_self() {
        let current = ModulePath::from_str_segments(&["foo", "bar"]).unwrap();
        let path = vec![Arc::from("sibling")];

        let resolved = current
            .resolve_relative(&ImportPrefix::Self_, &path)
            .unwrap();
        assert_eq!(resolved.to_string(), "foo.sibling");
    }

    #[test]
    fn test_resolve_self_from_root() {
        let current = ModulePath::from_str_segments(&["main"]).unwrap();
        let path = vec![Arc::from("sibling")];

        let resolved = current
            .resolve_relative(&ImportPrefix::Self_, &path)
            .unwrap();
        assert_eq!(resolved.to_string(), "sibling");
    }

    #[test]
    fn test_resolve_super() {
        let current = ModulePath::from_str_segments(&["a", "b", "c"]).unwrap();
        let path = vec![Arc::from("other")];

        let resolved = current
            .resolve_relative(&ImportPrefix::Super(1), &path)
            .unwrap();
        assert_eq!(resolved.to_string(), "a.other");
    }

    #[test]
    fn test_resolve_super_super() {
        let current = ModulePath::from_str_segments(&["a", "b", "c"]).unwrap();
        let path = vec![Arc::from("other")];

        let resolved = current
            .resolve_relative(&ImportPrefix::Super(2), &path)
            .unwrap();
        assert_eq!(resolved.to_string(), "other");
    }

    #[test]
    fn test_resolve_super_escapes_root() {
        let current = ModulePath::from_str_segments(&["a", "b"]).unwrap();
        let path = vec![Arc::from("other")];

        let err = current
            .resolve_relative(&ImportPrefix::Super(2), &path)
            .unwrap_err();
        assert!(matches!(err, ResolutionError::EscapedPackageRoot));
    }

    #[test]
    fn test_resolve_core() {
        let current = ModulePath::root();
        let path = vec![Arc::from("List")];

        let resolved = current
            .resolve_relative(&ImportPrefix::Core, &path)
            .expect("core paths resolve under the reserved `core` root");
        assert_eq!(resolved.to_string(), "core.List");
    }

    #[test]
    fn test_resolve_platform() {
        let current = ModulePath::root();
        let path = vec![Arc::from("Network")];

        let resolved = current
            .resolve_relative(&ImportPrefix::Platform, &path)
            .expect("platform paths resolve under the reserved `platform` root");
        assert_eq!(resolved.to_string(), "platform.Network");
    }

    #[test]
    fn test_resolve_empty_path() {
        let current = ModulePath::root();

        let err = current
            .resolve_relative(&ImportPrefix::Pkg, &[])
            .unwrap_err();
        assert!(matches!(err, ResolutionError::EmptyPath));
    }
}

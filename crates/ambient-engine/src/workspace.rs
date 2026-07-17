//! Multi-package workspaces.
//!
//! A workspace is a directory whose `ambient.toml` is a *workspace root*
//! (`[workspace] members = [...]`, see [`crate::manifest::ManifestFile`])
//! grouping several first-party packages:
//!
//! - Every member is an ordinary package (its own `ambient.toml` with
//!   `[package]`), listed by directory relative to the workspace root.
//! - Member package names are unique across the workspace — each name is
//!   the package's identity ([`Scope::Workspace`](crate::fqn::Scope)) and,
//!   in a workspace build, the leading module-path segment its modules are
//!   mounted under.
//! - Members reference each other with `use ::<package>::...`; libraries
//!   (content-addressed third-party dependencies) are explicitly out of
//!   scope — a workspace is first-party code only.
//! - The `.ambient` store lives at the *workspace* root and is shared by
//!   every member (see [`crate::disk_store`]).
//!
//! [`Workspace::discover`] is the single upward-walk authority: given any
//! path it finds the enclosing package (if any) and the enclosing
//! workspace (if any), so the CLI, build pipeline, and analysis layer all
//! agree on which packages participate in a build.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::manifest::{MANIFEST_FILENAME, Manifest, ManifestError, ManifestFile};

/// Errors that can occur when loading a workspace.
#[derive(Debug, Error)]
pub enum WorkspaceError {
    /// A manifest (the root's or a member's) failed to load.
    #[error("failed to load {path}: {source}")]
    Manifest {
        path: PathBuf,
        #[source]
        source: ManifestError,
    },

    /// A member directory has no `ambient.toml`.
    #[error("workspace member `{member}` has no {MANIFEST_FILENAME} (expected at {path})")]
    MemberNotFound { member: String, path: PathBuf },

    /// A member's `ambient.toml` is itself a workspace root — workspaces
    /// do not nest.
    #[error("workspace member `{member}` is itself a workspace root; workspaces do not nest")]
    NestedWorkspace { member: String },

    /// Two members share a package name.
    #[error(
        "duplicate package name `{name}` in workspace: members `{first}` and `{second}` \
         both use it; package names must be unique across a workspace"
    )]
    DuplicateName {
        name: String,
        first: String,
        second: String,
    },

    /// A member package takes a reserved name.
    #[error("workspace member `{member}` uses the reserved package name `{name}`")]
    ReservedName { member: String, name: String },

    /// A package lives under a workspace root that does not list it.
    #[error(
        "package `{package}` is inside workspace `{workspace}` but is not a member; \
         add it to [workspace] members"
    )]
    NotAMember {
        package: PathBuf,
        workspace: PathBuf,
    },
}

/// One member package of a workspace.
#[derive(Debug, Clone)]
pub struct WorkspaceMember {
    /// The package name (from the member's manifest) — its identity scope
    /// and its mount segment in workspace builds.
    pub name: String,
    /// The member's root directory (where its `ambient.toml` lives).
    pub root: PathBuf,
    /// The member's package manifest.
    pub manifest: Manifest,
}

/// A loaded workspace: the root directory plus every member package, in
/// manifest declaration order.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// The workspace root (where the `[workspace]` manifest lives).
    pub root: PathBuf,
    /// Every member package, in declaration order.
    pub members: Vec<WorkspaceMember>,
}

/// What [`Workspace::discover`] found enclosing a path.
#[derive(Debug)]
pub enum Discovered {
    /// The path is inside a standalone package (no enclosing workspace).
    Package(Manifest, PathBuf),
    /// The path is inside a member of a workspace. The `usize` indexes
    /// [`Workspace::members`].
    Member(Workspace, usize),
    /// The path is inside a workspace root but not inside any package
    /// (e.g. the root directory itself).
    WorkspaceRoot(Workspace),
    /// No manifest anywhere up the tree.
    None,
}

impl Workspace {
    /// Open a workspace whose root directory is already known.
    ///
    /// Loads and validates every member: each member directory must hold a
    /// package manifest, package names must be unique, and reserved names
    /// are rejected.
    ///
    /// # Errors
    ///
    /// Returns an error if the root manifest is not a workspace root or if
    /// any member fails validation.
    pub fn open(root: &Path) -> Result<Self, WorkspaceError> {
        let manifest_path = root.join(MANIFEST_FILENAME);
        let file =
            ManifestFile::from_file(&manifest_path).map_err(|source| WorkspaceError::Manifest {
                path: manifest_path.clone(),
                source,
            })?;
        let ManifestFile::Workspace(ws) = file else {
            return Err(WorkspaceError::Manifest {
                path: manifest_path,
                source: ManifestError::WorkspaceNotPackage,
            });
        };

        let mut members = Vec::with_capacity(ws.members.len());
        let mut seen: BTreeMap<String, String> = BTreeMap::new();
        for member_dir in &ws.members {
            let member_root = root.join(member_dir);
            let member_manifest_path = member_root.join(MANIFEST_FILENAME);
            if !member_manifest_path.is_file() {
                return Err(WorkspaceError::MemberNotFound {
                    member: member_dir.clone(),
                    path: member_manifest_path,
                });
            }
            let manifest = match ManifestFile::from_file(&member_manifest_path) {
                Ok(ManifestFile::Package(manifest)) => manifest,
                Ok(ManifestFile::Workspace(_)) => {
                    return Err(WorkspaceError::NestedWorkspace {
                        member: member_dir.clone(),
                    });
                }
                Err(source) => {
                    return Err(WorkspaceError::Manifest {
                        path: member_manifest_path,
                        source,
                    });
                }
            };
            // `core` is the reserved builtin namespace; a member named after
            // it could never be referenced (`core` is a keyword) and its
            // canonical names would collide with the standard library's.
            if manifest.name == "core" {
                return Err(WorkspaceError::ReservedName {
                    member: member_dir.clone(),
                    name: manifest.name,
                });
            }
            if let Some(first) = seen.insert(manifest.name.clone(), member_dir.clone()) {
                return Err(WorkspaceError::DuplicateName {
                    name: manifest.name,
                    first,
                    second: member_dir.clone(),
                });
            }
            members.push(WorkspaceMember {
                name: manifest.name.clone(),
                root: member_root,
                manifest,
            });
        }

        Ok(Self {
            root: root.to_path_buf(),
            members,
        })
    }

    /// Discover what encloses `path`: a standalone package, a workspace
    /// member, a bare workspace root, or nothing.
    ///
    /// Walks up from `path` to the nearest `ambient.toml`. A workspace-root
    /// manifest resolves immediately; a package manifest triggers a second
    /// walk further up looking for a workspace root — if one exists it must
    /// list the package as a member (a package under an unrelated workspace
    /// root is an error, never a silent standalone build with a split
    /// cache).
    ///
    /// # Errors
    ///
    /// Returns an error if a manifest fails to load or validate, or if the
    /// package is under a workspace that does not list it.
    pub fn discover(path: &Path) -> Result<Discovered, WorkspaceError> {
        let start = if path.is_file() {
            path.parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
        } else {
            path.to_path_buf()
        };

        let mut current = start.as_path();
        loop {
            let manifest_path = current.join(MANIFEST_FILENAME);
            if manifest_path.is_file() {
                let file = ManifestFile::from_file(&manifest_path).map_err(|source| {
                    WorkspaceError::Manifest {
                        path: manifest_path.clone(),
                        source,
                    }
                })?;
                return match file {
                    ManifestFile::Workspace(_) => {
                        Ok(Discovered::WorkspaceRoot(Self::open(current)?))
                    }
                    ManifestFile::Package(manifest) => Self::resolve_enclosing(current, manifest),
                };
            }
            match current.parent() {
                Some(parent) => current = parent,
                None => return Ok(Discovered::None),
            }
        }
    }

    /// The second walk of [`Self::discover`]: `package_root` holds a package
    /// manifest; find the enclosing workspace root (if any) and require the
    /// package to be one of its members.
    fn resolve_enclosing(
        package_root: &Path,
        manifest: Manifest,
    ) -> Result<Discovered, WorkspaceError> {
        let mut current = package_root;
        while let Some(parent) = current.parent() {
            current = parent;
            let manifest_path = current.join(MANIFEST_FILENAME);
            if !manifest_path.is_file() {
                continue;
            }
            let file = ManifestFile::from_file(&manifest_path).map_err(|source| {
                WorkspaceError::Manifest {
                    path: manifest_path.clone(),
                    source,
                }
            })?;
            // Only a workspace root claims packages below it; a *package*
            // manifest above another package is unrelated nesting — keep
            // walking (matching `Manifest::find`, which would already have
            // stopped at the inner package).
            if let ManifestFile::Workspace(_) = file {
                let workspace = Self::open(current)?;
                let canonical_pkg = normalize(package_root);
                return match workspace
                    .members
                    .iter()
                    .position(|m| normalize(&m.root) == canonical_pkg)
                {
                    Some(index) => Ok(Discovered::Member(workspace, index)),
                    None => Err(WorkspaceError::NotAMember {
                        package: package_root.to_path_buf(),
                        workspace: current.to_path_buf(),
                    }),
                };
            }
        }
        Ok(Discovered::Package(manifest, package_root.to_path_buf()))
    }

    /// The member with the given package name.
    #[must_use]
    pub fn member_named(&self, name: &str) -> Option<&WorkspaceMember> {
        self.members.iter().find(|m| m.name == name)
    }

    /// The member whose root directory contains `path`, if any.
    #[must_use]
    pub fn member_containing(&self, path: &Path) -> Option<&WorkspaceMember> {
        let path = normalize(path);
        self.members
            .iter()
            .find(|m| path.starts_with(normalize(&m.root)))
    }
}

/// Lexically normalize a path (resolve `.`/`..` components without touching
/// the filesystem) so member-root comparisons are not defeated by spelling
/// (`root/./member` vs `root/member`).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_package(root: &Path, name: &str) {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join(MANIFEST_FILENAME),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
        )
        .unwrap();
        fs::write(root.join("src/main.ab"), "fn run(): number { 0 }\n").unwrap();
    }

    fn write_workspace(root: &Path, members: &[&str]) {
        fs::create_dir_all(root).unwrap();
        let list = members
            .iter()
            .map(|m| format!("\"{m}\""))
            .collect::<Vec<_>>()
            .join(", ");
        fs::write(
            root.join(MANIFEST_FILENAME),
            format!("[workspace]\nmembers = [{list}]\n"),
        )
        .unwrap();
    }

    #[test]
    fn open_loads_members_in_order() {
        let dir = TempDir::new().unwrap();
        write_workspace(dir.path(), &["b_pkg", "a_pkg"]);
        write_package(&dir.path().join("a_pkg"), "alpha");
        write_package(&dir.path().join("b_pkg"), "beta");

        let ws = Workspace::open(dir.path()).unwrap();
        let names: Vec<&str> = ws.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["beta", "alpha"]);
    }

    #[test]
    fn duplicate_names_rejected() {
        let dir = TempDir::new().unwrap();
        write_workspace(dir.path(), &["one", "two"]);
        write_package(&dir.path().join("one"), "same");
        write_package(&dir.path().join("two"), "same");

        let err = Workspace::open(dir.path()).unwrap_err();
        assert!(matches!(err, WorkspaceError::DuplicateName { .. }));
    }

    #[test]
    fn reserved_core_name_rejected() {
        let dir = TempDir::new().unwrap();
        write_workspace(dir.path(), &["one"]);
        write_package(&dir.path().join("one"), "core");

        let err = Workspace::open(dir.path()).unwrap_err();
        assert!(matches!(err, WorkspaceError::ReservedName { .. }));
    }

    #[test]
    fn missing_member_rejected() {
        let dir = TempDir::new().unwrap();
        write_workspace(dir.path(), &["ghost"]);

        let err = Workspace::open(dir.path()).unwrap_err();
        assert!(matches!(err, WorkspaceError::MemberNotFound { .. }));
    }

    #[test]
    fn discover_finds_member_from_source_file() {
        let dir = TempDir::new().unwrap();
        write_workspace(dir.path(), &["app"]);
        write_package(&dir.path().join("app"), "app");

        let found = Workspace::discover(&dir.path().join("app/src/main.ab")).unwrap();
        let Discovered::Member(ws, index) = found else {
            panic!("expected a workspace member");
        };
        assert_eq!(ws.members[index].name, "app");
    }

    #[test]
    fn discover_standalone_package() {
        let dir = TempDir::new().unwrap();
        write_package(dir.path(), "solo");

        let found = Workspace::discover(dir.path()).unwrap();
        assert!(matches!(found, Discovered::Package(m, _) if m.name == "solo"));
    }

    #[test]
    fn discover_workspace_root() {
        let dir = TempDir::new().unwrap();
        write_workspace(dir.path(), &["app"]);
        write_package(&dir.path().join("app"), "app");

        let found = Workspace::discover(dir.path()).unwrap();
        assert!(matches!(found, Discovered::WorkspaceRoot(_)));
    }

    #[test]
    fn unlisted_package_under_workspace_is_an_error() {
        let dir = TempDir::new().unwrap();
        write_workspace(dir.path(), &["app"]);
        write_package(&dir.path().join("app"), "app");
        write_package(&dir.path().join("stray"), "stray");

        let err = Workspace::discover(&dir.path().join("stray")).unwrap_err();
        assert!(matches!(err, WorkspaceError::NotAMember { .. }));
    }

    #[test]
    fn discover_nothing() {
        let dir = TempDir::new().unwrap();
        // No manifest anywhere under a fresh temp dir; the walk stops at
        // the filesystem root. (If a stray manifest exists above the temp
        // dir this test environment is broken anyway.)
        let found = Workspace::discover(dir.path()).unwrap();
        assert!(matches!(found, Discovered::None | Discovered::Package(..)));
    }
}

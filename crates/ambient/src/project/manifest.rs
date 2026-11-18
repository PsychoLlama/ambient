//! What a manifest says: of a package, of a workspace, or of both. See
//! "Layout" and "Workspaces" in `spec/packages.md`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use super::locked::Source;
use super::version::Version;

/// A build step: a tool of the package, and the target it adds files
/// to builds under. Each is a path as a prelude's is, to a definition.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub target: String,
    pub tool: String,
}

/// What a package says of itself.
#[derive(Clone, Debug)]
pub struct Package {
    /// What the package is called in messages, and in this workspace.
    pub name: String,
    /// The package's identity, made once and kept whatever the package
    /// comes to be called. See "Identity" in `spec/packages.md`.
    pub id: String,
    pub version: Version,
    /// The modules in scope in every module of the package, each a path
    /// that begins with `pkg` or a dependency. Listing any replaces the
    /// standard library's.
    pub preludes: Vec<String>,
    /// Other packages, by the name this one uses for each.
    pub dependencies: BTreeMap<String, Dependency>,
    /// The abilities the package's comptime calls may take from the
    /// build target, each a path as a prelude's is, to an ability.
    pub grants: Vec<String>,
    /// The files and directories beside the sources that are part of
    /// the package, each a path from the manifest's directory.
    pub files: Vec<PathBuf>,
    /// The build steps the package adds to builds under a target. See
    /// "Builds" in `spec/tools.md`.
    pub steps: Vec<Step>,
    /// The lints checking the package runs, each a path as a
    /// prelude's is, to a function. See "Commands" in `spec/tools.md`.
    pub lints: Vec<String>,
}

/// A package another depends on: which it is, and where to find it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    /// The ID of the package meant.
    pub id: String,
    /// The least version of it that will do, whose line is the one
    /// meant. See "Selection" in `spec/releases.md`.
    pub version: Option<Version>,
    /// Where the package is, from the directory of this manifest. One
    /// that is nowhere is a member of the workspace.
    pub path: Option<PathBuf>,
    /// The git repository the package is in.
    pub git: Option<String>,
    /// The branch of it that is meant.
    pub branch: Option<String>,
    /// The tag of it that is meant.
    pub tag: Option<String>,
    /// The commit of it that is meant, written out whole.
    pub rev: Option<String>,
    /// The package's directory in the repository, if not its top.
    pub directory: Option<PathBuf>,
}

/// Whether a path names something under where it starts.
fn inside(path: &Path) -> bool {
    let part = |part| matches!(part, Component::Normal(_));
    !path.as_os_str().is_empty() && path.components().all(part)
}

impl Dependency {
    /// Where the package is, if it is in a git repository.
    #[must_use]
    pub fn source(&self) -> Option<Source> {
        Some(Source {
            git: self.git.clone()?,
            branch: self.branch.clone(),
            tag: self.tag.clone(),
            rev: self.rev.clone(),
            directory: self.directory.clone().unwrap_or_default(),
        })
    }

    /// What is wrong with how the dependency says where its package
    /// is, if anything.
    fn wrong(&self) -> Option<&'static str> {
        let whole = |rev: &String| {
            let digit = |byte: u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte);
            matches!(rev.len(), 40 | 64) && rev.bytes().all(digit)
        };
        let states = [&self.branch, &self.tag, &self.rev];
        let states = states.into_iter().flatten().count();
        let outside = self.directory.as_deref().is_some_and(|path| !inside(path));
        match &self.git {
            Some(_) if self.path.is_some() => Some("says both a `path` and a `git`"),
            // Nothing is selected among a repository's versions.
            Some(_) if self.version.is_some() => Some("says both a `version` and a `git`"),
            Some(_) if states > 1 => Some("says more than one of a `branch`, a `tag`, and a `rev`"),
            // A name that moves is a branch or a tag.
            Some(_) if self.rev.as_ref().is_some_and(|rev| !whole(rev)) => {
                Some("says a `rev` that is not a whole commit")
            }
            Some(_) if outside => Some("says a `directory` that is not in the repository"),
            // The package at a directory is a member, as it is.
            None if self.path.is_some() && self.version.is_some() => {
                Some("says both a `path` and a `version`")
            }
            None if states > 0 => Some("says a `branch`, a `tag`, or a `rev`, and no `git`"),
            None if self.directory.is_some() => Some("says a `directory` and no `git`"),
            None if self.path.is_none() && self.version.is_none() => {
                Some("says neither a `path`, a `version`, nor a `git`")
            }
            _ => None,
        }
    }
}

/// A manifest as it is written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    name: Option<String>,
    id: Option<String>,
    version: Option<Version>,
    #[serde(default)]
    preludes: Vec<String>,
    #[serde(default)]
    dependencies: BTreeMap<String, Dependency>,
    #[serde(default)]
    grants: Vec<String>,
    #[serde(default)]
    files: Vec<PathBuf>,
    #[serde(default)]
    steps: Vec<Step>,
    #[serde(default)]
    lints: Vec<String>,
    members: Option<Vec<PathBuf>>,
    #[serde(default)]
    vendored: BTreeMap<PathBuf, String>,
}

/// A manifest: a package's, a workspace's, or one that is both.
#[derive(Clone, Debug)]
pub struct Manifest {
    pub package: Option<Package>,
    /// The directories of a workspace's members, from the manifest's
    /// own, if it is a workspace's.
    pub members: Option<Vec<PathBuf>>,
    /// The snapshot each member that was vendored was copied from, by
    /// its directory. See "Vendoring" in `spec/releases.md`.
    pub vendored: BTreeMap<PathBuf, String>,
}

impl Manifest {
    /// Reads a manifest's text.
    ///
    /// # Errors
    ///
    /// The text is not a manifest, and why.
    pub fn read(text: &str) -> Result<Self, String> {
        let written: Written = toml::from_str(text).map_err(|error| error.message().to_owned())?;
        let (members, vendored) = (written.members, written.vendored);
        let package = match (written.name, written.id, written.version) {
            (Some(name), Some(id), Some(version)) => Some(Package {
                name,
                id,
                version,
                preludes: written.preludes,
                dependencies: written.dependencies,
                grants: written.grants,
                files: written.files,
                steps: written.steps,
                lints: written.lints,
            }),
            (None, None, None) if members.is_some() => None,
            (name, id, _) => {
                let missing = match (name, id) {
                    (None, _) => "name",
                    (_, None) => "id",
                    _ => "version",
                };
                return Err(format!("a package's manifest says its `{missing}`"));
            }
        };
        for (name, dependency) in package.iter().flat_map(|package| &package.dependencies) {
            if let Some(wrong) = dependency.wrong() {
                return Err(format!("the dependency `{name}` {wrong}"));
            }
        }
        let outside = |path: &&PathBuf| !inside(path);
        let mut files = package.iter().flat_map(|package| &package.files);
        if let Some(path) = files.find(outside) {
            return Err(format!(
                "the file `{}` is not under the package's directory",
                path.display()
            ));
        }
        Ok(Self {
            package,
            members,
            vendored,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Manifest;

    #[test]
    fn a_manifest_is_a_package_a_workspace_or_both() {
        let package = "name = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n";
        let both = format!("members = [\"b\"]\n{package}");
        let read = |text: &str| Manifest::read(text).map(|m| (m.package.is_some(), m.members));
        assert_eq!(read(package), Ok((true, None)));
        assert_eq!(read("members = []"), Ok((false, Some(Vec::new()))));
        assert_eq!(read(&both), Ok((true, Some(vec!["b".into()]))));
    }

    #[test]
    fn a_version_is_a_semantic_version() {
        let read = |version: &str| {
            Manifest::read(&format!(
                "name = \"a\"\nid = \"a\"\nversion = \"{version}\""
            ))
            .is_ok()
        };
        assert!(read("1.20.3") && read("0.1.0-rc.1") && read("1.0.0+linux"));
        for wrong in ["1.2", "1.2.3.4", "1.02.3", "1.2.x", ""] {
            assert!(!read(wrong), "{wrong}");
        }
    }

    #[test]
    fn a_manifest_that_is_neither_says_what_it_lacks() {
        let lacks = |text: &str| Manifest::read(text).err();
        let says = |what: &str| Some(format!("a package's manifest says its `{what}`"));
        assert_eq!(lacks(""), says("name"));
        assert_eq!(lacks("name = \"a\""), says("id"));
        assert_eq!(lacks("name = \"a\"\nid = \"a\""), says("version"));
    }

    #[test]
    fn a_file_a_package_lists_is_under_its_directory() {
        let package = "name = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n";
        let read = |files: &str| Manifest::read(&format!("{package}files = [{files}]")).is_ok();
        assert!(read("\"pages\", \"notes/today.txt\""));
        for outside in ["\"../secret\"", "\"/etc\"", "\"\"", "\"./pages\""] {
            assert!(!read(outside), "{outside}");
        }
    }

    #[test]
    fn a_dependency_says_where_it_is_or_which_version() {
        let package = "name = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n[dependencies]\n";
        let read = |dependency: &str| Manifest::read(&format!("{package}{dependency}")).err();
        assert_eq!(read("b = { id = \"b\", path = \"../b\" }"), None);
        assert_eq!(read("b = { id = \"b\", version = \"2.1.0\" }"), None);
        assert_eq!(
            read("b = { id = \"b\", version = \"2.1.0\", path = \"../b\" }"),
            Some("the dependency `b` says both a `path` and a `version`".to_owned())
        );
        assert_eq!(
            read("b = { id = \"b\" }"),
            Some("the dependency `b` says neither a `path`, a `version`, nor a `git`".to_owned())
        );
    }

    #[test]
    fn a_dependency_in_a_repository_says_at_most_one_state_of_it() {
        let package = "name = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n[dependencies]\n";
        let rev = "0123456789abcdef0123456789abcdef01234567";
        let read = |found: &str| {
            let found = found.replace("REV", rev);
            let read = Manifest::read(&format!("{package}b = {{ id = \"b\", {found} }}"));
            read.err()
                .map(|said| said.replace("the dependency `b` says ", ""))
        };
        let said = |wrong: &str| Some(wrong.to_owned());
        let git = "git = \"url\"";

        for right in ["", "branch = \"main\"", "tag = \"v1\"", "rev = \"REV\""] {
            let found = format!("{git}, directory = \"packages/b\", {right}");
            assert_eq!(read(&found), None, "{right}");
        }
        assert_eq!(
            read(&format!("{git}, branch = \"main\", rev = \"REV\"")),
            said("more than one of a `branch`, a `tag`, and a `rev`")
        );
        assert_eq!(
            read(&format!("{git}, rev = \"main\"")),
            said("a `rev` that is not a whole commit")
        );
        assert_eq!(
            read(&format!("{git}, path = \"../b\"")),
            said("both a `path` and a `git`")
        );
        assert_eq!(
            read(&format!("{git}, version = \"1.0.0\"")),
            said("both a `version` and a `git`")
        );
        assert_eq!(
            read("tag = \"v1\", path = \"../b\""),
            said("a `branch`, a `tag`, or a `rev`, and no `git`")
        );
        assert_eq!(
            read(&format!("{git}, directory = \"../b\"")),
            said("a `directory` that is not in the repository")
        );
    }
}

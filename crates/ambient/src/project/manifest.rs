//! What a manifest says: of a package, of a workspace, or of both. See
//! "Layout" and "Workspaces" in `docs/spec/packages.md`.

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

/// A native bundle: the machine code of a package's externs for one
/// platform, as a file the manifest lists by its digest. See
/// "Distribution" in `docs/spec/externs.md`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    /// The digest of its content, which is what is held to.
    pub digest: Option<String>,
    /// Somewhere to fetch it from: a hint.
    pub url: Option<String>,
    /// Where it is under the package's directory, for a member: what
    /// is there is used as it is.
    pub path: Option<PathBuf>,
    /// The libraries of the system it is to be linked with, each as
    /// the linker is told of one.
    #[serde(default)]
    pub libraries: Vec<String>,
}

impl Bundle {
    /// What is wrong with what it says, if anything is.
    fn wrong(&self) -> Option<&'static str> {
        let outside = self.path.as_deref().is_some_and(|path| !inside(path));
        match (&self.digest, &self.path) {
            (None, None) => Some("says neither a `digest` nor a `path`"),
            _ if outside => Some("says a `path` that is not under the package's directory"),
            _ => None,
        }
    }
}

/// What a package says of itself.
#[derive(Clone, Debug)]
pub struct Package {
    /// What the package is called in messages, and in this workspace.
    pub name: String,
    /// The package's identity, made once and kept whatever the package
    /// comes to be called. See "Identity" in `docs/spec/packages.md`.
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
    /// "Builds" in `docs/spec/tools.md`.
    pub steps: Vec<Step>,
    /// The lints checking the package runs, each a path as a
    /// prelude's is, to a function. See "Commands" in `docs/spec/tools.md`.
    pub lints: Vec<String>,
    /// Its native bundles, each by the platform it is for: what the
    /// native backend calls the machine.
    pub native: BTreeMap<String, Bundle>,
}

/// A package another depends on: where it is, or which it is. Only a
/// package that is nowhere is named by its ID, which every other says
/// in its own manifest. See "Dependencies" in `docs/spec/packages.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dependency {
    /// The package at a directory, from the directory of this
    /// manifest: a member of the workspace, as it is.
    Path(PathBuf),
    /// The package in a git repository.
    Git(Source),
    /// The package of an ID, which a member of the workspace is.
    Named {
        id: String,
        /// The least version of it that will do, whose line is the one
        /// meant. See "Selection" in `docs/spec/releases.md`.
        version: Version,
    },
}

impl Dependency {
    /// The directory the package is at, if the dependency names one.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Path(path) => Some(path),
            _ => None,
        }
    }

    /// Where the package is, if it is in a git repository.
    #[must_use]
    pub fn source(&self) -> Option<&Source> {
        match self {
            Self::Git(source) => Some(source),
            _ => None,
        }
    }
}

/// A dependency as it is written: every key any kind of one takes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Declared {
    id: Option<String>,
    version: Option<Version>,
    path: Option<PathBuf>,
    git: Option<String>,
    branch: Option<String>,
    tag: Option<String>,
    /// A commit, written out whole.
    rev: Option<String>,
    /// The package's directory in the repository, if not its top.
    directory: Option<PathBuf>,
}

/// Whether a path names something under where it starts.
fn inside(path: &Path) -> bool {
    let part = |part| matches!(part, Component::Normal(_));
    !path.as_os_str().is_empty() && path.components().all(part)
}

impl Declared {
    /// The dependency this says, or what is wrong with how it says
    /// where its package is.
    fn read(self) -> Result<Dependency, &'static str> {
        let states = [&self.branch, &self.tag, &self.rev];
        let states = states.into_iter().flatten().count();
        match (self.git, self.path) {
            (Some(_), Some(_)) => Err("says both a `path` and a `git`"),
            // Nothing is selected among a repository's versions.
            (Some(_), _) if self.version.is_some() => Err("says both a `version` and a `git`"),
            // The package there says its own.
            (Some(_), _) if self.id.is_some() => Err("says both an `id` and a `git`"),
            (Some(_), _) if states > 1 => {
                Err("says more than one of a `branch`, a `tag`, and a `rev`")
            }
            (Some(git), _) => {
                let source = Source {
                    git,
                    branch: self.branch,
                    tag: self.tag,
                    rev: self.rev,
                    directory: self.directory.unwrap_or_default(),
                };
                wrong(&source).map_or(Ok(Dependency::Git(source)), Err)
            }
            (None, _) if states > 0 => Err("says a `branch`, a `tag`, or a `rev`, and no `git`"),
            (None, _) if self.directory.is_some() => Err("says a `directory` and no `git`"),
            // The package at a directory is a member, as it is.
            (None, Some(_)) if self.version.is_some() => Err("says both a `path` and a `version`"),
            (None, Some(_)) if self.id.is_some() => Err("says both an `id` and a `path`"),
            (None, Some(path)) => Ok(Dependency::Path(path)),
            (None, None) => match (self.id, self.version) {
                (Some(id), Some(version)) => Ok(Dependency::Named { id, version }),
                (None, Some(_)) => Err("says a `version` and no `id`"),
                (_, None) => Err("says neither a `path`, a `version`, nor a `git`"),
            },
        }
    }
}

/// What is wrong with how a declaration says the state of its
/// repository, if anything.
fn wrong(source: &Source) -> Option<&'static str> {
    let digit = |byte: u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte);
    let whole = |rev: &String| matches!(rev.len(), 40 | 64) && rev.bytes().all(digit);
    let top = source.directory.as_os_str().is_empty();
    match &source.rev {
        // A name that moves is a branch or a tag.
        Some(rev) if !whole(rev) => Some("says a `rev` that is not a whole commit"),
        _ if !top && !inside(&source.directory) => {
            Some("says a `directory` that is not in the repository")
        }
        _ => None,
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
    dependencies: BTreeMap<String, Declared>,
    #[serde(default)]
    grants: Vec<String>,
    #[serde(default)]
    files: Vec<PathBuf>,
    #[serde(default)]
    steps: Vec<Step>,
    #[serde(default)]
    lints: Vec<String>,
    #[serde(default)]
    native: BTreeMap<String, Bundle>,
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
    /// its directory. See "Vendoring" in `docs/spec/releases.md`.
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
        let mut dependencies = BTreeMap::new();
        for (name, declared) in written.dependencies {
            let read = declared.read();
            let read = read.map_err(|wrong| format!("the dependency `{name}` {wrong}"))?;
            dependencies.insert(name, read);
        }
        let package = match (written.name, written.id, written.version) {
            (Some(name), Some(id), Some(version)) => Some(Package {
                name,
                id,
                version,
                preludes: written.preludes,
                dependencies,
                grants: written.grants,
                files: written.files,
                steps: written.steps,
                lints: written.lints,
                native: written.native,
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
        for (platform, bundle) in package.iter().flat_map(|package| &package.native) {
            if let Some(wrong) = bundle.wrong() {
                return Err(format!("the native bundle for `{platform}` {wrong}"));
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
    fn a_native_bundle_says_its_digest_or_where_it_is() {
        let package = "name = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n[native.linux]\n";
        let read = |bundle: &str| Manifest::read(&format!("{package}{bundle}")).err();
        let said = |wrong: &str| Some(format!("the native bundle for `linux` {wrong}"));
        assert_eq!(
            read("digest = \"d\"\nurl = \"u\"\nlibraries = [\"-lm\"]"),
            None
        );
        assert_eq!(read("path = \"native/a.o\""), None);
        assert_eq!(
            read("url = \"u\""),
            said("says neither a `digest` nor a `path`")
        );
        assert_eq!(
            read("path = \"../a.o\""),
            said("says a `path` that is not under the package's directory")
        );
    }

    #[test]
    fn a_dependency_says_where_it_is_or_which_version() {
        let package = "name = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n[dependencies]\n";
        let read = |dependency: &str| Manifest::read(&format!("{package}{dependency}")).err();
        let said = |wrong: &str| Some(format!("the dependency `b` says {wrong}"));
        assert_eq!(read("b = { path = \"../b\" }"), None);
        assert_eq!(read("b = { id = \"b\", version = \"2.1.0\" }"), None);
        assert_eq!(
            read("b = { version = \"2.1.0\", path = \"../b\" }"),
            said("both a `path` and a `version`")
        );
        assert_eq!(
            read("b = { version = \"2.1.0\" }"),
            said("a `version` and no `id`")
        );
        assert_eq!(
            read("b = { id = \"b\" }"),
            said("neither a `path`, a `version`, nor a `git`")
        );
    }

    #[test]
    fn a_dependency_that_says_where_its_package_is_says_no_id() {
        let package = "name = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n[dependencies]\n";
        let read =
            |found: &str| Manifest::read(&format!("{package}b = {{ id = \"b\", {found} }}")).err();
        let said = |wrong: &str| Some(format!("the dependency `b` says {wrong}"));
        assert_eq!(read("path = \"../b\""), said("both an `id` and a `path`"));
        assert_eq!(read("git = \"url\""), said("both an `id` and a `git`"));
    }

    #[test]
    fn a_dependency_in_a_repository_says_at_most_one_state_of_it() {
        let package = "name = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n[dependencies]\n";
        let rev = "0123456789abcdef0123456789abcdef01234567";
        let read = |found: &str| {
            let found = found.replace("REV", rev);
            let read = Manifest::read(&format!("{package}b = {{ {found} }}"));
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

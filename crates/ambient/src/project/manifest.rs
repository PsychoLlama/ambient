//! What a manifest says: of a package, of a workspace, or of both. See
//! "Layout" and "Workspaces" in `docs/spec/packages.md`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

use super::dependency::{Declared, Dependency, inside};
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
    /// The target a command at the package runs and builds under when
    /// it is told none: a path as a prelude's is, to a target. See
    /// "Running" in `docs/spec/packages.md`.
    pub target: Option<String>,
    /// Its native bundles, each by the platform it is for: what the
    /// native backend calls the machine.
    pub native: BTreeMap<String, Bundle>,
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
    target: Option<String>,
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
                target: written.target,
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

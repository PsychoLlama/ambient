//! What a manifest says: of a package, of a workspace, or of both. See
//! "Layout" and "Workspaces" in `spec/packages.md`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::locked::Pin;

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
}

/// A package another depends on: which it is, and where to find it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    /// The ID of the package meant.
    pub id: String,
    /// The least version of it that will do, whose major is the one
    /// meant. See "Selection" in `spec/releases.md`.
    pub version: Option<Version>,
    /// Where the package is, from the directory of this manifest. One
    /// that is nowhere is a member of the workspace.
    pub path: Option<PathBuf>,
    /// The repository a pinned package is in.
    pub git: Option<String>,
    /// The commit of it that is pinned, written out whole.
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
    /// Exactly what the dependency pins, if it pins.
    #[must_use]
    pub fn pin(&self) -> Option<Pin> {
        Some(Pin {
            git: self.git.clone()?,
            rev: self.rev.clone()?,
            directory: self.directory.clone().unwrap_or_default(),
        })
    }

    /// What is wrong with how the dependency says where its package
    /// is, if anything.
    fn wrong(&self) -> Option<&'static str> {
        // A name that moves is no pin: a commit is written out whole.
        let whole = |rev: &String| {
            let digit = |byte: u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte);
            matches!(rev.len(), 40 | 64) && rev.bytes().all(digit)
        };
        match (&self.git, &self.rev) {
            (Some(_), _) if self.path.is_some() => Some("says both a `path` and a `git`"),
            (Some(_), None) => Some("says a `git` and no `rev`"),
            (Some(_), Some(rev)) if !whole(rev) => Some("pins a `rev` that is not a whole commit"),
            (None, Some(_)) => Some("says a `rev` and no `git`"),
            (None, None) if self.directory.is_some() => Some("says a `directory` and no `git`"),
            (None, None) if self.path.is_none() && self.version.is_none() => {
                Some("says neither a `path`, a `version`, nor a `git`")
            }
            _ if self.directory.as_deref().is_some_and(|path| !inside(path)) => {
                Some("says a `directory` that is not in the repository")
            }
            _ => None,
        }
    }
}

/// A package's version: a major, a minor, and a patch. See "Versions"
/// in `spec/releases.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl TryFrom<String> for Version {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        // A number is written one way: no sign, no leading zero.
        let number = |part: &str| {
            let plain = part == "0" || !part.starts_with('0');
            let digits = !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
            (plain && digits).then(|| part.parse().ok()).flatten()
        };
        let mut parts = text.split('.').map(number);
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(Some(major)), Some(Some(minor)), Some(Some(patch)), None) => Ok(Self {
                major,
                minor,
                patch,
            }),
            _ => Err(format!(
                "`{text}` is not a version: three numbers, as in `1.2.0`"
            )),
        }
    }
}

impl From<Version> for String {
    fn from(version: Version) -> Self {
        version.to_string()
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
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
    use super::{Manifest, Version};

    fn version(text: &str) -> Result<Version, String> {
        Version::try_from(text.to_owned())
    }

    #[test]
    fn a_version_is_three_numbers_written_one_way() {
        let read = version("1.20.3").map(|version| version.to_string());
        assert_eq!(read.as_deref(), Ok("1.20.3"));
        for wrong in ["1.2", "1.2.3.4", "1.02.3", "1.2.x", "+1.2.3", "", "1..3"] {
            assert!(version(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn versions_are_ordered_by_major_then_minor_then_patch() {
        let ordered = ["0.9.9", "1.0.0", "1.0.10", "1.2.0", "2.0.0"].map(version);
        assert!(ordered.is_sorted());
    }

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
            read("b = { id = \"b\" }"),
            Some("the dependency `b` says neither a `path`, a `version`, nor a `git`".to_owned())
        );
    }

    #[test]
    fn a_dependency_pins_a_whole_commit_of_a_repository() {
        let package = "name = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n[dependencies]\n";
        let rev = "0123456789abcdef0123456789abcdef01234567";
        let read = |pinned: &str| {
            let pinned = pinned.replace("REV", rev);
            let read = Manifest::read(&format!("{package}b = {{ id = \"b\", {pinned} }}"));
            read.err()
                .map(|said| said.replace("the dependency `b` ", ""))
        };
        let said = |wrong: &str| Some(wrong.to_owned());

        assert_eq!(
            read("git = \"url\", rev = \"REV\", directory = \"packages/b\""),
            None
        );
        assert_eq!(read("git = \"url\""), said("says a `git` and no `rev`"));
        assert_eq!(
            read("git = \"url\", rev = \"main\""),
            said("pins a `rev` that is not a whole commit")
        );
        assert_eq!(
            read("git = \"url\", rev = \"REV\", path = \"../b\""),
            said("says both a `path` and a `git`")
        );
        assert_eq!(
            read("rev = \"REV\", path = \"../b\""),
            said("says a `rev` and no `git`")
        );
        assert_eq!(
            read("git = \"url\", rev = \"REV\", directory = \"../b\""),
            said("says a `directory` that is not in the repository")
        );
    }
}

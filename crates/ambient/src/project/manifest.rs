//! What a manifest says: of a package, of a workspace, or of both. See
//! "Layout" and "Workspaces" in `docs/spec/packages.md`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

use super::bins::Bin;
use super::dependency::{Declared, Dependency, Granted, inside};
use super::version::Version;

/// A build step: a tool of the package, and the target it adds files
/// to builds under. Each is a path as a prelude's is, to a definition.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub target: String,
    pub tool: String,
}

/// What a manifest says under `test`: the parts of testing the package
/// replaces. See `docs/spec/testing.md`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Testing {
    /// The functions that find the package's tests, each a path as a
    /// prelude's is. None is the standard library's way.
    #[serde(default)]
    pub find: Vec<String>,
    /// The function that runs the package's tests, a path as a
    /// prelude's is. None is the standard library's.
    pub run: Option<String>,
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
    /// What the package is, for whoever reads its manifest. No build
    /// reads it.
    pub description: Option<String>,
    /// The modules in scope in every module of the package, each a path
    /// that begins with `pkg` or a dependency. Listing any replaces the
    /// standard library's.
    pub preludes: Vec<String>,
    /// Other packages, by the name this one uses for each.
    pub dependencies: BTreeMap<String, Dependency>,
    /// What the package lets each dependency take, by the name it
    /// uses for it.
    pub granted: BTreeMap<String, Granted>,
    /// The files and directories beside the sources that are part of
    /// the package, each a path from the manifest's directory.
    pub files: Vec<PathBuf>,
    /// The build steps the package adds to builds under a target. See
    /// "Builds" in `docs/spec/tools.md`.
    pub steps: Vec<Step>,
    /// The lints checking the package runs, each a path as a
    /// prelude's is, to a function. See "Commands" in `docs/spec/tools.md`.
    pub lints: Vec<String>,
    /// The parts of testing the package replaces.
    pub test: Testing,
    /// The binaries the package declares, each by the name of its
    /// function. See "Binaries" in `docs/spec/packages.md`.
    pub bins: BTreeMap<String, Bin>,
    /// Its native bundles, each by the platform it is for: what the
    /// native backend calls the machine.
    pub native: BTreeMap<String, Bundle>,
}

/// What a manifest says under `package`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Said {
    name: Option<String>,
    id: Option<String>,
    version: Option<Version>,
    description: Option<String>,
    #[serde(default)]
    preludes: Vec<String>,
    #[serde(default)]
    files: Vec<PathBuf>,
    #[serde(default)]
    lints: Vec<String>,
}

/// What a manifest says under `workspace`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Listed {
    #[serde(default)]
    members: Vec<PathBuf>,
    #[serde(default)]
    vendored: BTreeMap<PathBuf, String>,
}

/// A manifest as it is written: a table for the package, one for the
/// workspace, and one for each kind of thing a package lists by name.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    package: Option<Said>,
    workspace: Option<Listed>,
    #[serde(default)]
    bin: BTreeMap<String, Bin>,
    #[serde(default)]
    dependencies: BTreeMap<String, Declared>,
    #[serde(default)]
    steps: Vec<Step>,
    #[serde(default)]
    native: BTreeMap<String, Bundle>,
    #[serde(default)]
    test: Testing,
}

impl Written {
    /// A table this has that only a package's manifest does, if any.
    fn of_a_package(&self) -> Option<&'static str> {
        let tables = [
            ("bin", self.bin.is_empty()),
            ("dependencies", self.dependencies.is_empty()),
            ("steps", self.steps.is_empty()),
            ("native", self.native.is_empty()),
            ("test", self.test == Testing::default()),
        ];
        let said = tables.into_iter().find(|(_, empty)| !empty);
        said.map(|(table, _)| table)
    }
}

/// A manifest: a package's, a workspace's, or one that is both.
#[derive(Clone, Debug)]
pub struct Manifest {
    pub package: Option<Package>,
    /// The members of a workspace as its manifest lists them, if it is
    /// a workspace's: each a directory from the manifest's own, or a
    /// pattern for several (`project/listed.rs`).
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
        let of_a_package = written.of_a_package();
        let (members, vendored) = match written.workspace {
            Some(listed) => (Some(listed.members), listed.vendored),
            None => (None, BTreeMap::new()),
        };
        let mut listed = members.iter().flatten();
        if let Some((member, wrong)) = listed.find_map(|at| Some((at, super::listed::wrong(at)?))) {
            return Err(format!("the member `{}` {wrong}", member.display()));
        }
        let Some(said) = written.package else {
            return match (of_a_package, &members) {
                (Some(table), _) => Err(format!(
                    "`{table}` is said of a package, and there is no `package`"
                )),
                (None, None) => {
                    Err("a manifest says a `package`, a `workspace`, or both".to_owned())
                }
                (None, Some(_)) => Ok(Self {
                    package: None,
                    members,
                    vendored,
                }),
            };
        };
        let (mut dependencies, mut granted) = (BTreeMap::new(), BTreeMap::new());
        for (name, declared) in written.dependencies {
            let read = declared.read();
            let (on, grants) = read.map_err(|wrong| format!("the dependency `{name}` {wrong}"))?;
            dependencies.insert(name.clone(), on);
            granted.insert(name, grants);
        }
        for (name, bin) in &written.bin {
            if let Some(wrong) = bin.wrong(name) {
                return Err(format!("the binary `{name}` {wrong}"));
            }
        }
        for (platform, bundle) in &written.native {
            if let Some(wrong) = bundle.wrong() {
                return Err(format!("the native bundle for `{platform}` {wrong}"));
            }
        }
        if let Some(path) = said.files.iter().find(|path| !inside(path)) {
            return Err(format!(
                "the file `{}` is not under the package's directory",
                path.display()
            ));
        }
        // Each is asked for by name: what is missing is what to write.
        let (name, id, version) = match (said.name, said.id, said.version) {
            (Some(name), Some(id), Some(version)) => (name, id, version),
            (name, id, _) => {
                let missing = match (name, id) {
                    (None, _) => "name",
                    (_, None) => "id",
                    _ => "version",
                };
                return Err(format!("a package's manifest says its `{missing}`"));
            }
        };
        let package = Package {
            name,
            id,
            version,
            description: said.description,
            preludes: said.preludes,
            dependencies,
            granted,
            files: said.files,
            steps: written.steps,
            lints: said.lints,
            test: written.test,
            bins: written.bin,
            native: written.native,
        };
        Ok(Self {
            package: Some(package),
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
        let package = "[package]\nname = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n";
        let both = format!("{package}[workspace]\nmembers = [\"b\"]");
        let read = |text: &str| Manifest::read(text).map(|m| (m.package.is_some(), m.members));
        assert_eq!(read(package), Ok((true, None)));
        assert_eq!(read("[workspace]"), Ok((false, Some(Vec::new()))));
        assert_eq!(read(&both), Ok((true, Some(vec!["b".into()]))));
    }

    #[test]
    fn a_version_is_a_semantic_version() {
        let read = |version: &str| {
            Manifest::read(&format!(
                "[package]\nname = \"a\"\nid = \"a\"\nversion = \"{version}\""
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
        let lacks = |text: &str| Manifest::read(text).unwrap_err();
        let neither = "a manifest says a `package`, a `workspace`, or both";
        assert_eq!(lacks(""), neither);
        let says = |what: &str| format!("a package's manifest says its `{what}`");
        assert_eq!(lacks("[package]"), says("name"));
        assert_eq!(lacks("[package]\nname = \"a\""), says("id"));
        assert_eq!(
            lacks("[package]\nname = \"a\"\nid = \"a\""),
            says("version")
        );
        assert_eq!(
            lacks("[workspace]\n[dependencies]\nb = { path = \"b\" }"),
            "`dependencies` is said of a package, and there is no `package`"
        );
    }

    #[test]
    fn a_binary_is_named_as_a_function_and_its_executable_as_a_file() {
        let package = "[package]\nname = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n";
        let read = |bin: &str| Manifest::read(&format!("{package}{bin}")).err();
        let said = |wrong: &str| Some(format!("the binary `{wrong}"));
        let whole = "[bin.main]\nname = \"a-tool\"\ntarget = \"pkg.web.page\"";
        assert_eq!(read(whole), None);
        assert_eq!(
            read("[bin.a-tool]"),
            said("a-tool` is not named as a function is")
        );
        assert_eq!(
            read("[bin.main]\nname = \"bin/a\""),
            said("main` says a `name` that is not the name of a file")
        );
        assert!(read("[bin.main]\nentry = \"a\"").is_some());
    }

    #[test]
    fn a_package_may_say_what_it_is() {
        let package = "[package]\nname = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n";
        let described = |rest: &str| {
            let read = Manifest::read(&format!("{package}{rest}")).unwrap();
            read.package.unwrap().description
        };
        assert_eq!(described(""), None);
        assert_eq!(
            described("description = \"Says hello\""),
            Some("Says hello".to_owned())
        );
    }

    #[test]
    fn a_member_is_a_directory_or_a_pattern_of_whole_names() {
        let read = |members: &str| Manifest::read(&format!("[workspace]\nmembers = [{members}]"));
        assert!(read("\"app\", \"libs/*\"").is_ok());
        assert_eq!(
            read("\"libs/**\"").unwrap_err(),
            "the member `libs/**` has a `*` that is not the whole name of a directory"
        );
    }

    #[test]
    fn a_file_a_package_lists_is_under_its_directory() {
        let package = "[package]\nname = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n";
        let read = |files: &str| Manifest::read(&format!("{package}files = [{files}]")).is_ok();
        assert!(read("\"pages\", \"notes/today.txt\""));
        for outside in ["\"../secret\"", "\"/etc\"", "\"\"", "\"./pages\""] {
            assert!(!read(outside), "{outside}");
        }
    }

    #[test]
    fn a_native_bundle_says_its_digest_or_where_it_is() {
        let package = "[package]\nname = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n[native.linux]\n";
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
        let package = "[package]\nname = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n[dependencies]\n";
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
        let package = "[package]\nname = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n[dependencies]\n";
        let read =
            |found: &str| Manifest::read(&format!("{package}b = {{ id = \"b\", {found} }}")).err();
        let said = |wrong: &str| Some(format!("the dependency `b` says {wrong}"));
        assert_eq!(read("path = \"../b\""), said("both an `id` and a `path`"));
        assert_eq!(read("git = \"url\""), said("both an `id` and a `git`"));
    }

    #[test]
    fn a_dependency_in_a_repository_says_at_most_one_state_of_it() {
        let package = "[package]\nname = \"a\"\nid = \"a\"\nversion = \"1.0.0\"\n[dependencies]\n";
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

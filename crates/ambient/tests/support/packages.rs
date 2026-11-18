//! Packages written out as files held in memory, for tests of how
//! they find each other.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ambient::cli::Command;

use super::scripted::Scripted;

pub type Files = BTreeMap<PathBuf, String>;

/// A package to write out: where it is, and what its manifest says.
pub struct Package {
    at: &'static str,
    manifest: String,
    library: &'static str,
}

/// A package at `at`, named for it, with the ID `id` at version 1.0.0.
pub fn package(at: &'static str, id: &str) -> Package {
    Package::versioned(at, id, "1.0.0")
}

impl Package {
    pub fn versioned(at: &'static str, id: &str, version: &str) -> Self {
        let name = at.rsplit('/').next().unwrap();
        let manifest =
            format!("name = \"{name}\"\nid = \"{id}\"\nversion = \"{version}\"\n[dependencies]\n");
        Self {
            at,
            manifest,
            library: "",
        }
    }

    /// Depends on the package `id` as `name`, `found` saying where it
    /// is or which version: `path = ".."` or `version = ".."`.
    pub fn on(mut self, name: &str, id: &str, found: &str) -> Self {
        let dependency = format!("{name} = {{ id = \"{id}\", {found} }}\n");
        self.manifest.push_str(&dependency);
        self
    }

    /// Has `source` for its library module.
    pub fn library(mut self, source: &'static str) -> Self {
        self.library = source;
        self
    }
}

/// The files of `packages`, of which one named `app` says its `main`
/// in the module `main`, under a workspace at the top that lists
/// `members`.
pub fn files<const N: usize>(members: Option<&[&str]>, packages: [Package; N]) -> Files {
    let mut files = Files::new();
    if let Some(members) = members {
        files.insert("ambient.toml".into(), workspace(members));
    }
    for package in packages {
        let at = PathBuf::from(package.at);
        let module = if at.ends_with("app") { "main" } else { "lib" };
        files.insert(at.join("ambient.toml"), package.manifest);
        files.insert(
            at.join(format!("src/{module}.ab")),
            package.library.to_owned(),
        );
    }
    files
}

/// The manifest of a workspace that lists `members`.
pub fn workspace(members: &[&str]) -> String {
    let listed: Vec<String> = members.iter().map(|at| format!("\"{at}\"")).collect();
    format!("members = [{}]", listed.join(", "))
}

/// What running `app` exits with, or the error it reports.
pub fn ran(files: &Files) -> Result<i32, String> {
    ran_at(files, "app")
}

/// What running the package at `at` exits with, or the error it
/// reports.
pub fn ran_at(files: &Files, at: &str) -> Result<i32, String> {
    let command = Command::Run {
        path: PathBuf::from(at),
        entry: "main.main".to_owned(),
        args: Vec::new(),
    };
    let (code, world) = Scripted::default().execute(files, &command);
    if world.err.is_empty() {
        Ok(code)
    } else {
        Err(world.err)
    }
}

/// `files` with what `command` wrote over them, or what it refused
/// with.
pub fn written(mut files: Files, command: &Command) -> Result<Files, String> {
    let (_, world) = Scripted::default().execute(&files, command);
    if !world.err.is_empty() {
        return Err(world.err);
    }
    for (path, bytes) in world.files {
        let text = String::from_utf8(bytes).unwrap();
        files.insert(PathBuf::from(path), text);
    }
    Ok(files)
}

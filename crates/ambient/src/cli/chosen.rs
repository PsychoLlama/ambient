//! Which packages a command is for: those at a directory, or the
//! members of the workspace there that it names. See "Workspaces" in
//! `docs/spec/packages.md`.

use std::path::PathBuf;

use clap::Args;

use super::Session;
use crate::project;

/// The packages a command is for.
#[derive(Clone, Debug, PartialEq, Eq, Args)]
pub struct Chosen {
    /// The directory of the package, or of its workspace.
    #[arg(default_value = ".")]
    pub path: PathBuf,
    /// The member of the workspace there that the command is for, by
    /// name.
    #[arg(long = "package", short, value_name = "NAME")]
    pub packages: Vec<String>,
}

impl Chosen {
    /// The package at `path`, or every member of the workspace there.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            packages: Vec::new(),
        }
    }

    /// The member called `package` of the workspace at the same
    /// directory, if one is named, and else what this is.
    #[must_use]
    pub fn of(&self, package: Option<&str>) -> Self {
        Self {
            path: self.path.clone(),
            packages: package.map(str::to_owned).into_iter().collect(),
        }
    }
}

/// What a command for one package says where it is told of several.
const SEVERAL: &str = "this is done for one package: name one with `--package`";
/// What it says at a workspace's directory that is no package.
const APART: &str = "this is a workspace and no package: name a member with `--package`";

impl Session<'_> {
    /// The directory of each package `chosen` is: the members it
    /// names, or with none named every package its directory covers.
    pub(super) fn each(&self, chosen: &Chosen) -> Result<Vec<PathBuf>, Vec<String>> {
        let refused = |error: project::ProjectError| vec![error.to_string()];
        if chosen.packages.is_empty() {
            return project::covered(self.files, &chosen.path).map_err(refused);
        }
        let named = |name: &String| project::named(self.files, &chosen.path, name);
        let named: Result<Vec<PathBuf>, _> = chosen.packages.iter().map(named).collect();
        named.map_err(refused)
    }

    /// The directory of the one package `chosen` is, for a command
    /// that is for one: the member it names, or else its directory.
    pub(super) fn one(&self, chosen: &Chosen) -> Result<PathBuf, Vec<String>> {
        match chosen.packages.as_slice() {
            [] if project::apart(self.files, &chosen.path) => {
                Err(vec![format!("{}: {APART}", chosen.path.display())])
            }
            [] => Ok(chosen.path.clone()),
            [_] => Ok(self.each(chosen)?.remove(0)),
            _ => Err(vec![SEVERAL.to_owned()]),
        }
    }
}

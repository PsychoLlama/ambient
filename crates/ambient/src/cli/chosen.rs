//! Which packages a command is for: the one it is run in, or the
//! members of the workspace there that it names. See "Commands" in
//! `docs/spec/packages.md`.

use std::path::PathBuf;

use clap::Args;

use super::Session;
use crate::project::{self, ProjectError};

/// The packages a command is for, among those where it is run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Args)]
pub struct Chosen {
    /// The member of the workspace that the command is for, by name;
    /// the package it is run in, if not said.
    #[arg(long = "package", short, value_name = "NAME")]
    pub packages: Vec<String>,
}

impl Chosen {
    /// The members called `names`.
    #[must_use]
    pub fn named(names: &[&str]) -> Self {
        Self {
            packages: names.iter().map(|name| (*name).to_owned()).collect(),
        }
    }
}

/// What a command for one package says where it is told of several.
const SEVERAL: &str = "this is done for one package: name one with `--package`";
/// What it says in a workspace's directory that is no package.
const APART: &str = "this is a workspace and no package: name a member with `--package`";

/// What a command reports of a workspace that could not be read.
#[allow(
    clippy::needless_pass_by_value,
    reason = "it is what `map_err` hands over"
)]
fn refused(error: ProjectError) -> Vec<String> {
    vec![error.to_string()]
}

impl Session<'_> {
    /// The directory of the package or the workspace the command is
    /// run in: the nearest one, there or above, that holds a manifest.
    fn home(&self) -> Result<PathBuf, Vec<String>> {
        project::nearest(self.files, self.directory).map_err(refused)
    }

    /// The directory of each package `chosen` is: the members it
    /// names, or with none named the package the command is run in,
    /// or every member of a workspace that is no package.
    pub(super) fn each(&self, chosen: &Chosen) -> Result<Vec<PathBuf>, Vec<String>> {
        let home = self.home()?;
        if chosen.packages.is_empty() {
            return project::covered(self.files, &home).map_err(refused);
        }
        // A member named twice is one package all the same.
        let mut named = Vec::new();
        for name in &chosen.packages {
            let member = project::named(self.files, &home, name).map_err(refused)?;
            if !named.contains(&member) {
                named.push(member);
            }
        }
        Ok(named)
    }

    /// The directory of the one package `chosen` is, for a command
    /// that is for one: the member it names, or else the package the
    /// command is run in.
    pub(super) fn one(&self, chosen: &Chosen) -> Result<PathBuf, Vec<String>> {
        let home = self.home()?;
        match chosen.packages.as_slice() {
            [] if project::apart(self.files, &home) => Err(vec![APART.to_owned()]),
            [] => Ok(home),
            [_] => Ok(self.each(chosen)?.remove(0)),
            _ => Err(vec![SEVERAL.to_owned()]),
        }
    }
}

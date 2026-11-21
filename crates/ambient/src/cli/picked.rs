//! Which binaries a command is for: the one it names, or what the
//! packages it is for have. See "Running" and "Building" in
//! `docs/spec/packages.md`.

use std::fmt::Write;
use std::path::PathBuf;

use super::{Chosen, Session};
use crate::project::{Binary, MAIN, Project};

/// A package a command is for, opened.
pub(super) struct Offered {
    pub(super) directory: PathBuf,
    pub(super) project: Project,
}

/// A binary a command is for, and the package it is of.
#[derive(Clone, Copy)]
pub(super) struct Picked<'a> {
    pub(super) package: &'a Offered,
    pub(super) binary: &'a Binary,
}

impl Offered {
    /// Where what is built of the package is put: the directory
    /// `build` of its workspace, or its own if no workspace lists it.
    pub(super) fn built(&self) -> PathBuf {
        let workspace = self.project.above.as_ref().unwrap_or(&self.directory);
        workspace.join(BUILT)
    }
}

/// The directory of a workspace that what is built is put in.
const BUILT: &str = "build";

/// What is said where no package a command is for has a binary.
const NONE: &str = "there is no binary here: a package has one where its `src/main.ab` \
     defines `main`, or where its manifest declares it under `[bin.<name>]`";

/// Every binary of `offered`, with the name it is declared under.
fn all(offered: &[Offered]) -> Vec<(&str, Picked<'_>)> {
    let mut all = Vec::new();
    for package in offered {
        for (name, binary) in &package.project.bins {
            all.push((name.as_str(), Picked { package, binary }));
        }
    }
    all
}

/// What refuses a choice that cannot be made, and what there is to
/// choose from: the binaries of each package, by the package.
fn refused(offered: &[Offered], why: &str) -> Vec<String> {
    let mut said = String::new();
    for package in offered
        .iter()
        .filter(|package| !package.project.bins.is_empty())
    {
        let names: Vec<&str> = package.project.bins.keys().map(String::as_str).collect();
        let _ = write!(said, "\n  {}: {}", package.project.root, names.join(", "));
    }
    if said.is_empty() {
        return vec![NONE.to_owned()];
    }
    vec![format!("{why}. There are:{said}")]
}

/// The binary called `name` of `offered`: an empty name is `--bin`
/// with nothing after it, which asks what there is.
fn named<'a>(offered: &'a [Offered], name: &str) -> Result<Picked<'a>, Vec<String>> {
    let called = |(key, _): &(&str, Picked)| *key == name;
    let found: Vec<_> = all(offered).into_iter().filter(called).collect();
    match found.as_slice() {
        [(_, picked)] => Ok(*picked),
        _ if name.is_empty() => Err(refused(offered, "`--bin` takes the name of a binary")),
        [] => Err(refused(
            offered,
            &format!("no binary here is called `{name}`"),
        )),
        _ => {
            let why =
                format!("more than one package has a binary `{name}`: say which with `--package`");
            Err(refused(offered, &why))
        }
    }
}

/// The one binary of `offered` a command told `bin` is for. Told
/// none, it is the only one there is, or `main` of the one package.
pub(super) fn one<'a>(
    offered: &'a [Offered],
    bin: Option<&str>,
) -> Result<Picked<'a>, Vec<String>> {
    if let Some(name) = bin {
        return named(offered, name);
    }
    let main = |package: &'a Offered| {
        let binary = package.project.bins.get(MAIN)?;
        Some(Picked { package, binary })
    };
    match (all(offered).as_slice(), offered) {
        ([(_, only)], _) => Ok(*only),
        (_, [package]) if main(package).is_some() => main(package).ok_or_else(Vec::new),
        _ => Err(refused(offered, "say which binary with `--bin`")),
    }
}

/// Each binary of `offered` a command told `bin` is for: that one, or
/// told none every one there is.
pub(super) fn every<'a>(
    offered: &'a [Offered],
    bin: Option<&str>,
) -> Result<Vec<Picked<'a>>, Vec<String>> {
    if let Some(name) = bin {
        return Ok(vec![named(offered, name)?]);
    }
    let every: Vec<_> = all(offered).into_iter().map(|(_, picked)| picked).collect();
    if every.is_empty() {
        return Err(vec![NONE.to_owned()]);
    }
    Ok(every)
}

/// The first two of `picked` that would be built as one thing: two
/// packages' binaries whose executables are called the same.
pub(super) fn rivals<'a>(picked: &[Picked<'a>]) -> Option<(Picked<'a>, Picked<'a>)> {
    let same = |one: &Picked, other: &Picked| one.binary.name == other.binary.name;
    let rival = |(index, one): (usize, &Picked<'a>)| {
        let other = picked[..index].iter().find(|other| same(one, other))?;
        Some((*other, *one))
    };
    picked.iter().enumerate().find_map(rival)
}

impl Session<'_> {
    /// Opens each package `chosen` is.
    pub(super) fn offered(&self, chosen: &Chosen) -> Result<Vec<Offered>, Vec<String>> {
        let open = |directory: PathBuf| {
            let project = self.open(&directory)?;
            Ok(Offered { directory, project })
        };
        self.each(chosen)?.into_iter().map(open).collect()
    }
}

//! The snapshots this machine keeps, each in a directory named for its
//! hash, and fetching one from a repository. See "Fetching" in
//! `spec/releases.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

use ambient_host::hash::Hash;

use super::members::manifest;
use super::{Commit, Contents, Disk, MANIFEST, Snapshots, Source};

/// Snapshots kept under one directory of this machine.
pub struct Kept {
    root: PathBuf,
}

/// What `git` says when run in `directory`. It never asks anyone
/// anything.
fn git(directory: &Path, args: &[&str]) -> Result<String, String> {
    let ran = Command::new("git")
        .args(args)
        .current_dir(directory)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| format!("git did not run: {error}"))?;
    if ran.status.success() {
        return Ok(String::from_utf8_lossy(&ran.stdout).into_owned());
    }
    let said = String::from_utf8_lossy(&ran.stderr);
    Err(format!("git {}: {}", args.join(" "), said.trim_end()))
}

impl Kept {
    #[must_use]
    pub fn under(root: PathBuf) -> Self {
        Self { root }
    }

    fn place(&self, snapshot: &Hash) -> PathBuf {
        self.root.join(snapshot.to_string())
    }

    /// Checks `commit` out into `work`.
    fn checkout(work: &Path, commit: Commit) -> Result<(), String> {
        git(work, &["init", "--quiet"])?;
        // Only the one commit, where the repository will give it; all
        // of the repository where it will not.
        let one = ["fetch", "--quiet", "--depth", "1", commit.git, commit.rev];
        if git(work, &one).is_err() {
            let all = "+refs/*:refs/fetched/*";
            git(work, &["fetch", "--quiet", commit.git, all])?;
        }
        git(work, &["checkout", "--quiet", "--detach", commit.rev]).map(|_| ())
    }

    /// Keeps the package at `directory`: its files and nothing else,
    /// in a directory that is whole or not there.
    fn keep(&self, directory: &Path) -> Result<Hash, String> {
        let said = manifest(&Disk, directory).map_err(|error| error.to_string())?;
        let said = said.package.ok_or("it is no package")?;
        let contents = Contents {
            directory: directory.to_owned(),
            listed: said.files,
        };
        let failed = |error: std::io::Error| error.to_string();
        let snapshot = contents.snapshot(&Disk).map_err(failed)?;
        let place = self.place(&snapshot);
        if place.join(MANIFEST).exists() {
            return Ok(snapshot);
        }
        let partial = place.with_extension(format!("{}.partial", std::process::id()));
        for (_, file) in contents.files(&Disk).map_err(failed)? {
            let to = partial.join(&file);
            std::fs::create_dir_all(to.parent().unwrap_or(&partial)).map_err(failed)?;
            std::fs::copy(directory.join(&file), to).map_err(failed)?;
        }
        // Another process may have kept it meanwhile, which is as good.
        let moved = std::fs::rename(&partial, &place);
        if moved.is_err() {
            std::fs::remove_dir_all(&partial).ok();
        }
        Ok(snapshot)
    }
}

impl Snapshots for Kept {
    fn held(&self, snapshot: &Hash) -> Option<PathBuf> {
        let place = self.place(snapshot);
        place.join(MANIFEST).exists().then_some(place)
    }

    fn resolve(&self, source: &Source) -> Result<String, String> {
        // A tag that is an object of its own is listed twice, the
        // second time as the commit it is of.
        let (named, refs) = match (&source.branch, &source.tag) {
            (Some(branch), _) => (
                format!("branch `{branch}`"),
                vec![format!("refs/heads/{branch}")],
            ),
            (None, Some(tag)) => (
                format!("tag `{tag}`"),
                vec![format!("refs/tags/{tag}"), format!("refs/tags/{tag}^{{}}")],
            ),
            (None, None) => ("branch of its own".to_owned(), vec!["HEAD".to_owned()]),
        };
        std::fs::create_dir_all(&self.root).map_err(|error| error.to_string())?;
        let mut args = vec!["ls-remote", &source.git];
        args.extend(refs.iter().map(String::as_str));
        let listed =
            git(&self.root, &args).map_err(|reason| format!("{}: {reason}", source.git))?;
        let commit = |line: &str| line.split_whitespace().next().map(str::to_owned);
        let found = listed.lines().last().and_then(commit);
        found.ok_or_else(|| format!("{} has no {named}", source.git))
    }

    fn fetch(&self, commit: Commit) -> Result<Hash, String> {
        let work = self.root.join(format!("fetching.{}", std::process::id()));
        std::fs::remove_dir_all(&work).ok();
        std::fs::create_dir_all(&work).map_err(|error| error.to_string())?;
        let kept =
            Self::checkout(&work, commit).and_then(|()| self.keep(&work.join(commit.directory)));
        std::fs::remove_dir_all(&work).ok();
        kept.map_err(|reason| format!("{}: {reason}", commit.git))
    }
}

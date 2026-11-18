//! The snapshots this machine keeps, each in a directory named for its
//! hash, and fetching one from a repository. See "Fetching" in
//! `spec/releases.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

use ambient_host::hash::Hash;

use super::members::manifest;
use super::{Contents, Disk, MANIFEST, Pin, Snapshots};

/// Snapshots kept under one directory of this machine.
pub struct Kept {
    root: PathBuf,
}

/// Runs `git` in `directory`, which never asks anyone anything.
fn git(directory: &Path, args: &[&str]) -> Result<(), String> {
    let ran = Command::new("git")
        .args(args)
        .current_dir(directory)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| format!("git did not run: {error}"))?;
    if ran.status.success() {
        return Ok(());
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

    /// Checks the commit `pin` names out into `work`.
    fn checkout(work: &Path, pin: &Pin) -> Result<(), String> {
        git(work, &["init", "--quiet"])?;
        // Only the one commit, where the repository will give it; all
        // of the repository where it will not.
        let one = ["fetch", "--quiet", "--depth", "1", &pin.git, &pin.rev];
        if git(work, &one).is_err() {
            let all = "+refs/*:refs/fetched/*";
            git(work, &["fetch", "--quiet", &pin.git, all])?;
        }
        git(work, &["checkout", "--quiet", "--detach", &pin.rev])
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

    fn fetch(&self, pin: &Pin) -> Result<Hash, String> {
        let work = self.root.join(format!("fetching.{}", std::process::id()));
        std::fs::remove_dir_all(&work).ok();
        std::fs::create_dir_all(&work).map_err(|error| error.to_string())?;
        let kept = Self::checkout(&work, pin).and_then(|()| self.keep(&work.join(&pin.directory)));
        std::fs::remove_dir_all(&work).ok();
        kept.map_err(|reason| format!("{}: {reason}", pin.git))
    }
}

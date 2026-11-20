//! The linker of this machine: what joins native bundles with the
//! launcher for a real build. See "Native code" in
//! `docs/spec/linking.md`.
//!
//! What it makes is kept under one directory of the machine, by what
//! it was made from, so a program is linked again only when what it
//! reaches of its bundles changes. None of it reaches a store: an
//! executable is no input to anything.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, io};

use ambient_host::hash::Hasher;
use ambient_host::parts::BUILD;

use super::linking::{ARCHIVE, LIBRARIES, Link, Linker};

/// The machine's own linker, and where what it makes is kept.
pub struct Machine {
    root: PathBuf,
}

/// Writes `bytes` at `path` whole or not at all: another process may
/// be writing the same file, which is as good.
fn keep(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    let partial = path.with_extension(format!("{}.partial", std::process::id()));
    fs::write(&partial, bytes)?;
    fs::rename(&partial, path).or_else(|_| fs::remove_file(&partial))
}

/// What tells the linker to leave out what nothing reaches.
const PRUNE: &str = if cfg!(target_vendor = "apple") {
    "-Wl,-dead_strip"
} else {
    "-Wl,--gc-sections"
};

impl Machine {
    #[must_use]
    pub fn under(root: PathBuf) -> Self {
        Self { root }
    }

    /// Where the executable of `link` is kept: a directory named for
    /// everything it is made from.
    fn place(&self, link: &Link) -> io::Result<PathBuf> {
        let mut hasher = Hasher::new("link");
        hasher.bytes(BUILD.as_bytes()).bytes(link.main.as_bytes());
        for bundle in &link.bundles {
            hasher.bytes(&fs::read(bundle)?);
        }
        for library in &link.libraries {
            hasher.bytes(library.as_bytes());
        }
        Ok(self.root.join(hasher.finish().to_string()))
    }

    /// Links what `link` describes into `place`.
    fn linked(&self, link: &Link, place: &Path) -> Result<(), String> {
        let failed = |error: io::Error| error.to_string();
        fs::create_dir_all(place).map_err(failed)?;
        let archive = self.root.join(format!("launcher-{BUILD}.a"));
        keep(&archive, ARCHIVE).map_err(failed)?;
        let main = place.join("main.c");
        fs::write(&main, &link.main).map_err(failed)?;
        let made = place.join(format!("launcher.{}.partial", std::process::id()));

        let linker = env::var_os("CC").unwrap_or_else(|| "cc".into());
        let mut command = Command::new(&linker);
        // A bundle refers to the launcher's runtime, so the archive is
        // read after it.
        command.arg(&main).args(&link.bundles).arg(&archive);
        command.args(link.libraries.iter().chain(&[PRUNE.to_owned()]));
        command
            .args(LIBRARIES.split_whitespace())
            .arg("-o")
            .arg(&made);
        let ran = command.output().map_err(|error| {
            format!(
                "the linker `{}` did not run: {error}",
                linker.to_string_lossy()
            )
        })?;
        if !ran.status.success() {
            let said = String::from_utf8_lossy(&ran.stderr);
            return Err(format!(
                "linking native bundles failed:\n{}",
                said.trim_end()
            ));
        }
        fs::rename(&made, place.join("launcher")).map_err(failed)
    }
}

impl Linker for Machine {
    fn link(&self, link: &Link) -> Result<Vec<u8>, String> {
        let failed = |error: io::Error| error.to_string();
        let place = self.place(link).map_err(failed)?;
        let launcher = place.join("launcher");
        if !launcher.exists() {
            self.linked(link, &place)?;
        }
        fs::read(launcher).map_err(failed)
    }

    fn run(&self, executable: &[u8], args: &[String]) -> Result<i32, String> {
        let failed = |error: io::Error| error.to_string();
        let name = Hasher::new("run").bytes(executable).finish().to_string();
        let place = self.root.join("running");
        fs::create_dir_all(&place).map_err(failed)?;
        let file = place.join(name);
        keep(&file, executable).map_err(failed)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).map_err(failed)?;
        }
        let status = Command::new(&file).args(args).status().map_err(failed)?;
        Ok(status.code().unwrap_or(1))
    }
}

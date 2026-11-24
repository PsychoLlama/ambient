//! The linker of this machine: what joins a `main` and native bundles
//! with the launcher for a real build. See "Native code" in
//! `docs/spec/linking.md`.
//!
//! What it makes is kept under one directory of the machine, by what
//! it was made from, so a program is linked again only when what it
//! reaches of the native side or of its bundles changes. None of it reaches a store: an
//! executable is no input to anything.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, io};

use std::sync::OnceLock;

use ambient_host::hash::{Hash, Hasher};

use super::linking::{Launcher, Link, Linker};
use super::sealing::seal;

/// The machine's own linker, and where what it makes is kept.
pub struct Machine {
    root: PathBuf,
    launcher: Launcher,
    /// The digest of the launcher, which names what is kept of it.
    named: OnceLock<Hash>,
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

/// What tells the linker to leave out the names of what it joined, as
/// a release of the launcher itself has none.
const STRIP: &[&str] = if cfg!(debug_assertions) {
    &[]
} else if cfg!(target_vendor = "apple") {
    &["-Wl,-x"]
} else {
    &["-s"]
};

impl Machine {
    /// What links with `launcher`, keeping what it makes under `root`.
    #[must_use]
    pub fn under(root: PathBuf, launcher: Launcher) -> Self {
        Self {
            root,
            launcher,
            named: OnceLock::new(),
        }
    }

    /// The digest of the launcher: of all it holds, the compiler and
    /// the host both, so what was made with another is never reused.
    fn named(&self) -> Hash {
        let digest = || {
            let mut hasher = Hasher::new("launcher");
            hasher
                .bytes(self.launcher.packed)
                .bytes(self.launcher.libraries.as_bytes());
            hasher.finish()
        };
        *self.named.get_or_init(digest)
    }

    /// Where the executable of `link` is kept: a directory named for
    /// everything it is made from.
    fn place(&self, link: &Link) -> io::Result<PathBuf> {
        let mut hasher = Hasher::new("link");
        hasher.hash(&self.named()).bytes(link.main.as_bytes());
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
        let archive = self.root.join(format!("launcher-{}.a", self.named()));
        if !archive.exists() {
            keep(&archive, &self.launcher.archive().map_err(failed)?).map_err(failed)?;
        }
        let main = place.join("main.c");
        fs::write(&main, &link.main).map_err(failed)?;
        let made = place.join(format!("launcher.{}.partial", std::process::id()));

        let linker = env::var_os("CC").unwrap_or_else(|| "cc".into());
        // Each bundle is sealed first, so that it shares only the
        // names of its externs with the launcher and the others.
        let sealed: Vec<PathBuf> = (0..link.bundles.len())
            .map(|number| place.join(format!("bundle-{number}.{}.o", std::process::id())))
            .collect();
        for (bundle, sealed) in link.bundles.iter().zip(&sealed) {
            seal(&linker, bundle, sealed)?;
        }
        let mut command = Command::new(&linker);
        // A bundle refers to the launcher's runtime, so the archive is
        // read after it.
        command.arg(&main).args(&sealed).arg(&archive);
        command.args(link.libraries.iter().chain(&[PRUNE.to_owned()]));
        command.args(STRIP);
        command
            .args(self.launcher.libraries.split_whitespace())
            .arg("-o")
            .arg(&made);
        let ran = command.output().map_err(|error| {
            format!(
                "the linker `{}` did not run: {error}\nan executable is linked by the machine's C \
                 compiler: install one, or name it in `CC`",
                linker.to_string_lossy()
            )
        })?;
        // A sealed bundle is as large as all its author linked in, and
        // is of no use once it is joined.
        for sealed in &sealed {
            let _ = fs::remove_file(sealed);
        }
        if !ran.status.success() {
            let said = String::from_utf8_lossy(&ran.stderr);
            return Err(format!("linking failed:\n{}", said.trim_end()));
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

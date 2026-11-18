//! The files that are a package, and the hash of them. See "Identity"
//! and "Layout" in `spec/packages.md`.

use std::io;
use std::path::{Component, Path, PathBuf};

use ambient_host::hash::{Hash, Hasher};

use super::{Files, MANIFEST, SOURCES};

/// The files of one package: its manifest, what is under its sources,
/// and what its manifest lists.
#[derive(Clone, Debug)]
pub struct Contents {
    /// Where the package is.
    pub directory: PathBuf,
    /// The files and directories its manifest lists, from there.
    pub listed: Vec<PathBuf>,
}

/// `path` as a snapshot names it: the same on every machine.
fn named(path: &Path) -> io::Result<String> {
    let mut name = String::new();
    for part in path.components() {
        let part = match part {
            Component::Normal(part) => part.to_str(),
            _ => None,
        };
        let unnamed = || io::Error::other(format!("{}: no snapshot names it", path.display()));
        name.push_str(part.ok_or_else(unnamed)?);
        name.push('/');
    }
    name.pop();
    Ok(name)
}

impl Contents {
    /// The path under the package's directory that `path` names, if it
    /// is of a file of the package, or of a directory of them.
    #[must_use]
    pub fn holds(&self, path: &Path) -> Option<PathBuf> {
        let mut inside = PathBuf::new();
        for part in path.components() {
            match part {
                Component::Normal(part) => inside.push(part),
                Component::CurDir => {}
                _ => return None,
            }
        }
        let listed = self.listed.iter().any(|listed| inside.starts_with(listed));
        let held = listed || inside.starts_with(SOURCES) || inside == Path::new(MANIFEST);
        held.then_some(inside)
    }

    /// Every file of the package, as a path from its directory, in the
    /// order their names are in.
    fn files(&self, files: &dyn Files) -> io::Result<Vec<(String, PathBuf)>> {
        let mut found = vec![PathBuf::from(MANIFEST)];
        let within = self.listed.iter().map(PathBuf::as_path);
        for within in within.chain([Path::new(SOURCES)]) {
            // What is listed is a directory of files, or one file.
            let under = files.sources(&self.directory.join(within));
            let under: Vec<PathBuf> = under.unwrap_or_default();
            let under = under.iter().filter(|path| !path.as_os_str().is_empty());
            let before = found.len();
            found.extend(under.map(|path| within.join(path)));
            if found.len() == before && within != Path::new(SOURCES) {
                found.push(within.to_owned());
            }
        }
        let named = |path: PathBuf| Ok((named(&path)?, path));
        let mut found = found
            .into_iter()
            .map(named)
            .collect::<io::Result<Vec<_>>>()?;
        found.sort();
        found.dedup();
        Ok(found)
    }

    /// The package's snapshot: a hash of each of its files, by name.
    ///
    /// # Errors
    ///
    /// A file is missing, cannot be read, or has a name that is not
    /// text.
    pub fn snapshot(&self, files: &dyn Files) -> io::Result<Hash> {
        let mut hasher = Hasher::new("snapshot");
        for (name, path) in self.files(files)? {
            let path = self.directory.join(path);
            let bytes = files.bytes(&path).map_err(|error| {
                io::Error::new(error.kind(), format!("{}: {error}", path.display()))
            })?;
            hasher.bytes(name.as_bytes()).bytes(&bytes);
        }
        Ok(hasher.finish())
    }
}

//! Where a project's files are read from.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Files, as a project reads them.
pub trait Files {
    /// The text of a file.
    ///
    /// # Errors
    ///
    /// The file is missing or is not text.
    fn read(&self, path: &Path) -> io::Result<String>;

    /// Every file under a directory, at any depth, as paths from it, in
    /// order.
    ///
    /// # Errors
    ///
    /// The directory is missing or cannot be read.
    fn sources(&self, directory: &Path) -> io::Result<Vec<PathBuf>>;

    /// The names of what a directory holds, in order.
    ///
    /// # Errors
    ///
    /// The directory is missing or cannot be read.
    fn names(&self, directory: &Path) -> io::Result<Vec<String>>;

    /// `path` from the top of these files, however it was written, so
    /// that two paths of one file can be told to be.
    fn absolute(&self, path: &Path) -> PathBuf;

    /// These files, for something that outlives a borrow of them: a
    /// build reads them whenever a comptime call asks.
    fn shared(&self) -> Rc<dyn Files>;
}

/// The files of this machine.
pub struct Disk;

impl Disk {
    fn walk(root: &Path, under: &Path, found: &mut Vec<PathBuf>) -> io::Result<()> {
        for entry in std::fs::read_dir(root.join(under))? {
            let entry = entry?;
            let path = under.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                Self::walk(root, &path, found)?;
            } else {
                found.push(path);
            }
        }
        Ok(())
    }
}

impl Files for Disk {
    fn read(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn sources(&self, directory: &Path) -> io::Result<Vec<PathBuf>> {
        let mut found = Vec::new();
        Self::walk(directory, Path::new(""), &mut found)?;
        found.sort();
        Ok(found)
    }

    fn names(&self, directory: &Path) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(directory)? {
            names.push(entry?.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        Ok(names)
    }

    fn absolute(&self, path: &Path) -> PathBuf {
        std::path::absolute(path).unwrap_or_else(|_| path.to_owned())
    }

    fn shared(&self) -> Rc<dyn Files> {
        Rc::new(Self)
    }
}

/// Files held in memory, by path.
impl Files for BTreeMap<PathBuf, String> {
    fn read(&self, path: &Path) -> io::Result<String> {
        let missing = || io::Error::new(io::ErrorKind::NotFound, "no such file");
        self.get(path).cloned().ok_or_else(missing)
    }

    fn sources(&self, directory: &Path) -> io::Result<Vec<PathBuf>> {
        let under = |path: &PathBuf| path.strip_prefix(directory).ok().map(Path::to_owned);
        Ok(self.keys().filter_map(under).collect())
    }

    fn names(&self, directory: &Path) -> io::Result<Vec<String>> {
        let first = |path: PathBuf| Some(path.iter().next()?.to_string_lossy().into_owned());
        let mut names: Vec<String> = self
            .sources(directory)?
            .into_iter()
            .filter_map(first)
            .collect();
        names.dedup();
        if names.is_empty() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no such directory"));
        }
        Ok(names)
    }

    fn absolute(&self, path: &Path) -> PathBuf {
        path.to_owned()
    }

    fn shared(&self) -> Rc<dyn Files> {
        Rc::new(self.clone())
    }
}
